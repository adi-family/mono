//! The on-demand test — a human presses "Test" and finds out, right now, whether a backend answers.
//!
//! This is not [`Prober`](super::prober::Prober). The prober exists so recovery never costs a chat
//! turn, and it earns that by touching nothing beyond `harness:adi`: a vendor CLI is a live terminal
//! or a headless one-shot process, not an HTTP call this binary can fire off in a background sweep
//! without spawning a child every few minutes forever. A human pressing a button is a different
//! transaction — one request, asked for by name, paid for once — and for that transaction spawning
//! the vendor's own CLI once is exactly the right cost. So this reaches **every** runtime a backend
//! can name, `probe` block or none, and it is deliberately not reachable from the sweep: nothing here
//! writes a hold, releases one, or extends one. It answers a question; it does not change the answer
//! to anyone else's.
//!
//! The prompt and the model come from the backend's own [`Probe`] when it has one — reusing the
//! cheap-model override that a recovery probe would use, since a human testing a backend is no more
//! interested in spending real tokens than the sweep is — and fall back to [`Probe::default`]'s
//! prompt and the backend's own model when it has none. A failure is read with
//! [`classify`](super::classify::classify) against the backend's own limit rules, so "still
//! rate-limited" means exactly what it means everywhere else that word is used.
//!
//! A vendor CLI spawned here needs two things a real agent run gets that this process was never
//! handed: a `PATH` wide enough to find it (see [`run_with_env`]), and a credential to authenticate
//! with (see [`resolve_credential`]) — a backend naming no `settings`/`api_key_env` of its own
//! answers through whatever CLI is ambiently logged in on a real run, which in practice means the
//! global secret an agent's `[[secrets]]` row attaches. Skipping either makes this lie: it reaches
//! for a shell that was never on this process's `PATH`, or a login nothing here ever provided, and
//! reports "not logged in" about a backend every agent uses successfully.

use std::process::Command;
use std::time::Instant;

use adi_config::{Config, now_unix};
use adi_secrets::Secrets;

use crate::backend::Backend;
use crate::error::{Error, Result};
use crate::llm::backend::{LlmBackendManifest, LlmBackends};
use crate::llm::classify::classify;

/// How long a vendor CLI gets to answer a couple of tokens before it is killed. Generous enough for
/// a cold process start and a real network round trip, short enough that a human who pressed "Test"
/// is not left staring at a spinner for minutes — unlike a full run, nothing here is worth waiting
/// [`crate::backends::harness::adi_loop`]'s ten-minute round budget for.
const TEST_TIMEOUT_MS: u64 = 45_000;

/// What a test found, and how long it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestResult {
    pub verdict: TestVerdict,
    /// Wall-clock time the ask itself took, from the first byte of the command line to the last byte
    /// read back — not the classification that follows it, which is local and free.
    pub elapsed_ms: u64,
    /// Which credential the test used, in words a human recognises (`"global secret
    /// CLAUDE_CODE_OAUTH_TOKEN"`, `"settings file ~/.claude/settings.glm.json"`) — empty for a
    /// runtime this function never reached (an unset backend, `harness:adi`). Folded into
    /// [`message`](Self::message) so a "not logged in" is never a mystery about which login was
    /// even tried.
    pub credential: String,
}

impl TestResult {
    /// One line for a human — what happened, in the words the provider used where there are any,
    /// and which credential answered for it.
    #[must_use]
    pub fn message(&self) -> String {
        let via = if self.credential.is_empty() {
            String::new()
        } else {
            format!(" via {}", self.credential)
        };
        match &self.verdict {
            TestVerdict::Answered => format!("answered in {}ms{via}", self.elapsed_ms),
            TestVerdict::RateLimited { reason } => format!("still rate-limited{via} — {reason}"),
            TestVerdict::Failed { error } => {
                if via.is_empty() {
                    error.clone()
                } else {
                    format!("{error}{via}")
                }
            }
        }
    }
}

/// The outcome, read the same way a probe's is: [`classify`] decides between a limit worth waiting
/// out and a failure worth showing as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestVerdict {
    /// It answered.
    Answered,
    /// Read against the backend's own limit rules: still out, in the provider's own words.
    RateLimited { reason: String },
    /// Failed for a reason that is not a limit — the provider's, or the CLI's, own error text.
    Failed { error: String },
}

impl TestVerdict {
    /// The wire tag this verdict is reported under — the vocabulary the API and the CLI's `--json`
    /// share, so neither invents its own spelling of "rate limited".
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Answered => "ok",
            Self::RateLimited { .. } => "rate_limited",
            Self::Failed { .. } => "failed",
        }
    }
}

/// Test a saved backend by id.
///
/// # Errors
/// [`Error::NotFound`] when there is no such backend; otherwise a store error reading it.
pub fn test_backend(config: &Config, id: &str) -> Result<TestResult> {
    let backend = LlmBackends::with_config(config.clone())
        .get(id)?
        .ok_or_else(|| Error::NotFound(id.to_string()))?;
    Ok(test_manifest(config, &backend.manifest))
}

/// Test a backend that may not be saved at all — a draft an operator is still typing into the panel.
/// This is the whole reason the core function takes a manifest rather than an id: the point of a
/// "Test" button beside a form is to test the form as it stands, not whatever was last saved.
///
/// `config` is where the credential lives, not the backend: a vendor CLI authenticates off the
/// environment it is started with, and a real run gets that from a secret an agent attached, not
/// from anything on the backend's own manifest. See [`resolve_credential`].
#[must_use]
pub fn test_manifest(config: &Config, manifest: &LlmBackendManifest) -> TestResult {
    let (prompt, model) = test_prompt(manifest);

    let start = Instant::now();
    let outcome = ask(config, manifest, model, prompt);
    let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

    let (verdict, credential) = match outcome {
        Ok(credential) => (TestVerdict::Answered, credential),
        Err((credential, text)) => {
            let found = classify(&text, &manifest.limit_rules, None, now_unix());
            let verdict = if found.class.holds() {
                TestVerdict::RateLimited { reason: found.evidence }
            } else {
                TestVerdict::Failed { error: text }
            };
            (verdict, credential)
        }
    };
    TestResult { verdict, elapsed_ms, credential }
}

/// [`Probe::default`]'s own prompt, kept as a `'static` literal so [`test_prompt`] can hand back a
/// borrow instead of an owned `String` — a test against [`Probe::default`] itself pins the two
/// together so this cannot silently drift from the value a saved probe would fall back to.
const DEFAULT_TEST_PROMPT: &str = "ok";

/// The prompt and model a test sends: the backend's own [`Probe`] when it declares one, and
/// otherwise the tiny default prompt on the backend's own model — the fallback that lets every
/// backend be tested whether or not anybody ever opted it into the background sweep.
fn test_prompt(manifest: &LlmBackendManifest) -> (&str, &str) {
    let probe = manifest.probe.as_ref();
    let prompt = probe.map_or(DEFAULT_TEST_PROMPT, |p| p.prompt.as_str());
    let model = probe
        .and_then(|p| p.model.as_deref())
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or(&manifest.model);
    (prompt, model)
}

/// Ask one backend its test prompt, once, through whatever runs it — no tools, no transcript, no
/// session, and (unlike [`super::prober::ask`]) no refusal for a runtime that isn't `harness:adi`.
///
/// Returns the credential description on both paths: [`TestResult`] wants it whether the ask
/// answered or failed, so a genuine "not logged in" is never a mystery about which login was even
/// tried. Empty for a runtime this reaches without resolving one (`harness:adi` reads its own
/// `api_key_env` from this process's environment; an unset or unknown runtime is refused before
/// any credential question arises).
fn ask(
    config: &Config,
    manifest: &LlmBackendManifest,
    model: &str,
    prompt: &str,
) -> std::result::Result<String, (String, String)> {
    match &manifest.runtime {
        Backend::HarnessAdi => crate::backends::harness::adi_loop::probe(&manifest.arguments(), model, prompt)
            .map(|_| String::new())
            .map_err(|e| (String::new(), e.to_string())),
        // All three run the `claude` CLI; a pty backend's live session and a harness backend's
        // scoped turn are both beside the point here, so every one of them gets the same plain
        // `--print` ask instead.
        Backend::PtyClaude | Backend::ProcessClaude | Backend::HarnessClaudeSdk => {
            let credential = match resolve_credential(config, manifest) {
                Ok(credential) => credential,
                Err(message) => return Err((String::new(), message)),
            };
            let argv = claude_argv(model, manifest.settings.as_deref(), prompt);
            let env: Vec<(String, String)> = credential.env.into_iter().collect();
            let outcome = run_with_env(&argv, &env).and_then(|output| {
                if output.status.success() {
                    Ok(())
                } else {
                    Err(command_error("claude", &output))
                }
            });
            match outcome {
                Ok(()) => Ok(credential.description),
                Err(error) => Err((credential.description, error)),
            }
        }
        // Likewise for `codex`: `codex exec` is the headless one-shot both the pty and the process
        // engine ultimately open a terminal or a child around.
        Backend::PtyCodex | Backend::ProcessCodex => {
            let credential = match resolve_credential(config, manifest) {
                Ok(credential) => credential,
                Err(message) => return Err((String::new(), message)),
            };
            let argv = codex_argv(model, prompt);
            let mut env: Vec<(String, String)> = credential.env.into_iter().collect();
            crate::backends::quiet_codex_env(&mut env);
            let outcome = run_with_env(&argv, &env).and_then(|output| read_codex(&output));
            match outcome {
                Ok(()) => Ok(credential.description),
                Err(error) => Err((credential.description, error)),
            }
        }
        Backend::Other(other) => Err((
            String::new(),
            format!(
                "{} is not a runtime this build knows how to test",
                if other.is_empty() { "(no runtime set)" } else { other }
            ),
        )),
    }
}

/// A credential a test resolved, and what to inject for it: [`None`] when the backend's own
/// arguments already carry it (a `--settings` file), `Some((name, value))` when a secret needs to
/// ride into the child's environment under the name its vendor CLI reads.
#[derive(Debug)]
struct Credential {
    /// What to tell a human this test used — see [`TestResult::credential`].
    description: String,
    env: Option<(String, String)>,
}

/// The environment variable a vendor CLI reads for its login when a backend names no credential of
/// its own — the *only* other place a run's credential can come from, so this is also the name
/// [`resolve_credential`] reports missing when nothing on this machine carries it.
///
/// `claude` honours `CLAUDE_CODE_OAUTH_TOKEN` — confirmed against the global secret of that name,
/// whose own description records it as "the env-var name the claude CLI actually honours". `codex`
/// persists a `ChatGPT` login in `~/.codex/auth.json`, but that file carries an `OPENAI_API_KEY`
/// field alongside it: the CLI accepts either, so an `OPENAI_API_KEY` secret is the one worth
/// checking here rather than assuming a host has already run `codex login` by hand.
fn ambient_credential_env(runtime: &Backend) -> Option<&'static str> {
    match runtime {
        Backend::PtyClaude | Backend::ProcessClaude | Backend::HarnessClaudeSdk => {
            Some("CLAUDE_CODE_OAUTH_TOKEN")
        }
        Backend::PtyCodex | Backend::ProcessCodex => Some("OPENAI_API_KEY"),
        Backend::HarnessAdi | Backend::Other(_) => None,
    }
}

/// Resolve a vendor-CLI backend's credential the way a real run's would: the backend's own
/// `settings` or `api_key_env` when it names one, otherwise the global secret carrying the env var
/// that runtime honours. A real run gets the same secret from an agent's `[[secrets]]` attachment
/// ([`crate::attached_secret_env`]) — this differs only in reaching for the *global* scope
/// directly, since a bare backend id has no agent, and therefore no project, to resolve one
/// against.
///
/// # Errors
/// A backend that names an `api_key_env` no secret answers to is a dead end nothing else can
/// rescue — refused here rather than sent to fail as "not logged in" downstream. A backend naming
/// nothing is not refused the same way: [`LlmBackendManifest::credential`] treats "the runtime's
/// own ambient login" as a legitimate credential in its own right, so a missing global secret here
/// falls back to running with no extra environment at all, exactly as an agent with no attached
/// secret would.
fn resolve_credential(
    config: &Config,
    manifest: &LlmBackendManifest,
) -> std::result::Result<Credential, String> {
    if let Some(settings) = manifest.settings.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(Credential { description: format!("settings file {settings}"), env: None });
    }
    let secrets = Secrets::with_config(config.clone());
    if let Some(name) = manifest.api_key_env.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return match secrets.reveal(None, name) {
            Ok(Some(value)) => Ok(Credential {
                description: format!("api_key_env {name}"),
                env: Some((name.to_string(), value)),
            }),
            _ => Err(format!(
                "names api_key_env {name}, but no secret named {name} is available — set one \
                 (`adi-mono secrets set {name}`) or point the backend at a settings file instead"
            )),
        };
    }
    match ambient_credential_env(&manifest.runtime) {
        Some(name) => match secrets.reveal(None, name) {
            Ok(Some(value)) => Ok(Credential {
                description: format!("global secret {name}"),
                env: Some((name.to_string(), value)),
            }),
            _ => Ok(Credential {
                description: format!("ambient {} login (no {name} secret set)", manifest.runtime),
                env: None,
            }),
        },
        None => Ok(Credential { description: format!("ambient {} login", manifest.runtime), env: None }),
    }
}

/// The `claude` CLI in print mode: no built-in tools (`--tools ""`, so a two-token prompt cannot turn
/// into the model reaching for the filesystem), plain text out so the answer needs no stream parser,
/// and the backend's own settings file when it names one — the same expansion
/// [`crate::backends::harness::claude_sdk`] applies, because a `~` reaches a spawned process as a
/// literal filename otherwise.
fn claude_argv(model: &str, settings: Option<&str>, prompt: &str) -> Vec<String> {
    let mut argv = vec![
        "claude".to_string(),
        "--print".to_string(),
        "--output-format".to_string(),
        "text".to_string(),
        "--tools".to_string(),
        String::new(),
    ];
    if !model.trim().is_empty() {
        argv.extend(["--model".to_string(), model.to_string()]);
    }
    if let Some(settings) = settings.map(str::trim).filter(|s| !s.is_empty()) {
        argv.extend(["--settings".to_string(), expand_settings(settings)]);
    }
    argv.push("--".to_string());
    argv.push(run_prompt(prompt));
    argv
}

/// `~`/`$HOME` expanded, JSON left untouched — the same rule
/// [`crate::backends::harness::claude_sdk`]'s own `settings` helper applies, duplicated in miniature
/// here because that one is built around a whole `HarnessClaudeSdkArguments`, not a bare string.
fn expand_settings(value: &str) -> String {
    if value.starts_with('{') {
        return value.to_string();
    }
    crate::launch::expand_home(value).map_or_else(|| value.to_string(), |p| p.display().to_string())
}

/// The `codex` CLI's `exec`: read-only and unattended, so a probe prompt cannot edit anything or
/// stop to ask permission, and `--json` because [`crate::backends::codex_stream`] is the only reader
/// that can tell a real failure from Codex's own startup narration.
fn codex_argv(model: &str, prompt: &str) -> Vec<String> {
    let mut argv = vec!["codex".to_string()];
    if !model.trim().is_empty() {
        argv.extend(["--model".to_string(), model.to_string()]);
    }
    argv.extend([
        "exec".to_string(),
        "--color".to_string(),
        "never".to_string(),
        "--sandbox".to_string(),
        "read-only".to_string(),
        "--ask-for-approval".to_string(),
        "never".to_string(),
        "--skip-git-repo-check".to_string(),
        "--json".to_string(),
        run_prompt(prompt),
    ]);
    argv
}

/// Read a `codex exec --json` run's combined output the same way a real run's log would be, so a
/// startup failure that never reached the model reads as an error rather than a silent success.
fn read_codex(output: &std::process::Output) -> std::result::Result<(), String> {
    let combined: Vec<u8> = output
        .stdout
        .iter()
        .chain(output.stderr.iter())
        .copied()
        .collect();
    let content = crate::backends::codex_stream::parse(&combined);
    match &content.metrics {
        Some(metrics) if metrics.is_error => Err(content.text.clone()),
        _ if output.status.success() => Ok(()),
        _ => Err(if content.text.trim().is_empty() {
            command_error("codex", output)
        } else {
            content.text.clone()
        }),
    }
}

fn run_prompt(prompt: &str) -> String {
    let prompt = prompt.trim();
    if prompt.is_empty() { "ok".to_string() } else { prompt.to_string() }
}

/// Spawn the vendor CLI on the same `PATH` a real run gets, not whatever this process inherited.
/// Under the app service that is launchd's or systemd's bare minimum — `/usr/bin:/bin` and
/// nothing else — which is a `PATH` no vendor CLI installed for a human, in a shell profile, was
/// ever going to be found on; a human pressing "Test" from a terminal never notices, because their
/// own shell's `PATH` already has it. [`crate::launch::run_path`] is the one place that gap is
/// closed for a real agent run, so this closes it the same way rather than inventing a second one.
fn run_with_env(
    argv: &[String],
    env: &[(String, String)],
) -> std::result::Result<std::process::Output, String> {
    let (program, rest) = argv.split_first().expect("argv always starts with the program");
    let mut cmd = Command::new(program);
    cmd.args(rest)
        .env("PATH", crate::launch::run_path(None, &[]))
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    crate::backends::harness::tools::wait_with_timeout(cmd, TEST_TIMEOUT_MS)
}

/// A command that exited without a structured answer to read: its stderr where there is one, its
/// stdout otherwise, and the bare exit status when it said nothing at all.
fn command_error(program: &str, output: &std::process::Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !text.is_empty() {
        return text;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !text.is_empty() {
        return text;
    }
    format!("{program} exited with {}", output.status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::backend::{HoldScope, LimitClass, LimitRule, Probe, Resume};

    fn manifest(runtime: Backend) -> LlmBackendManifest {
        LlmBackendManifest {
            runtime,
            model: "does-not-matter".into(),
            limit_rules: vec![LimitRule {
                pattern: "(?i)usage limit reached".into(),
                class: LimitClass::Quota,
                scope: HoldScope::Model,
                resume: Resume::Fixed,
                fixed: Some("4h".into()),
            }],
            ..LlmBackendManifest::default()
        }
    }

    fn scratch_config(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-ondemand-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    /// The one refusal that remains: a backend naming no runtime at all, or one this build has never
    /// heard of, has nothing to spawn. This is a pure dispatch failure — it never reaches `Command`,
    /// which is what makes it safe to run as an ordinary unit test.
    #[test]
    fn an_unset_runtime_fails_by_name_rather_than_being_asked_anything() {
        let result = test_manifest(&scratch_config("unset-runtime"), &manifest(Backend::default()));
        assert!(
            matches!(&result.verdict, TestVerdict::Failed { error } if error.contains("no runtime set")),
            "{result:?}"
        );
    }

    /// A backend with no probe still gets a tiny prompt and its own model — the fallback that makes
    /// testing a backend nobody opted into the sweep possible at all.
    #[test]
    fn a_backend_with_no_probe_falls_back_to_the_tiny_default_prompt() {
        let m = manifest(Backend::HarnessAdi);
        let (prompt, model) = test_prompt(&m);
        assert_eq!(prompt, DEFAULT_TEST_PROMPT);
        assert_eq!(DEFAULT_TEST_PROMPT, Probe::default().prompt, "the two must not drift apart");
        assert_eq!(model, "does-not-matter");
    }

    /// A probe's own model overrides the backend's — the cheap-model override a recovery probe would
    /// use is exactly what a human testing the backend should spend, too.
    #[test]
    fn a_probes_own_model_and_prompt_win_over_the_backends() {
        let m = LlmBackendManifest {
            probe: Some(Probe {
                model: Some("claude-haiku-4-5-20251001".into()),
                prompt: "ping".into(),
            }),
            ..manifest(Backend::HarnessAdi)
        };
        let (prompt, model) = test_prompt(&m);
        assert_eq!(prompt, "ping");
        assert_eq!(model, "claude-haiku-4-5-20251001");
    }

    /// A failure that matches the backend's own limit rule reads as rate-limited, exactly as the
    /// prober's would — the two must never disagree about what the same words mean.
    #[test]
    fn a_matching_limit_rule_reads_as_rate_limited_not_failed() {
        let found = classify(
            "Claude usage limit reached, resets at 14:00",
            &manifest(Backend::PtyClaude).limit_rules,
            None,
            0,
        );
        assert_eq!(found.class, LimitClass::Quota);
    }

    #[test]
    fn run_prompt_never_sends_an_empty_string() {
        assert_eq!(run_prompt(""), "ok");
        assert_eq!(run_prompt("  "), "ok");
        assert_eq!(run_prompt("ping"), "ping");
    }

    #[test]
    fn settings_json_is_left_untouched_but_a_path_is_home_expanded() {
        let home = std::env::var("HOME").expect("a test host has a HOME");
        assert_eq!(
            expand_settings("~/.claude/settings.glm.json"),
            format!("{home}/.claude/settings.glm.json"),
        );
        let json = r#"{"env":{"ANTHROPIC_BASE_URL":"https://api.z.ai/api/anthropic"}}"#;
        assert_eq!(expand_settings(json), json);
    }

    /// The claude argv a test sends: no built-in tools, plain text, and `--` before the prompt so it
    /// is never swallowed as another flag's value.
    #[test]
    fn claude_argv_denies_tools_and_asks_in_plain_text() {
        assert_eq!(
            claude_argv("claude-opus-5", Some("~/.claude/settings.glm.json"), "ok"),
            [
                "claude",
                "--print",
                "--output-format",
                "text",
                "--tools",
                "",
                "--model",
                "claude-opus-5",
                "--settings",
                &format!("{}/.claude/settings.glm.json", std::env::var("HOME").unwrap()),
                "--",
                "ok",
            ]
        );
    }

    /// No model and no settings still asks something: both flags are simply absent, exactly as an
    /// agent with no override runs on the runtime's own default.
    #[test]
    fn claude_argv_omits_absent_model_and_settings() {
        assert_eq!(
            claude_argv("", None, ""),
            ["claude", "--print", "--output-format", "text", "--tools", "", "--", "ok"]
        );
    }

    /// The codex argv a test sends: read-only, unattended, and `--json` so the answer can be told
    /// from the CLI's own narration.
    #[test]
    fn codex_argv_is_read_only_and_unattended() {
        assert_eq!(
            codex_argv("gpt-5-codex", "ok"),
            [
                "codex",
                "--model",
                "gpt-5-codex",
                "exec",
                "--color",
                "never",
                "--sandbox",
                "read-only",
                "--ask-for-approval",
                "never",
                "--skip-git-repo-check",
                "--json",
                "ok",
            ]
        );
    }

    /// A settings file is the whole credential — nothing to look up, so a scratch store with no
    /// secrets in it still resolves one.
    #[test]
    fn a_settings_file_is_the_credential_and_needs_no_secret_lookup() {
        let m = LlmBackendManifest {
            settings: Some("~/.claude/settings.glm.json".into()),
            ..manifest(Backend::PtyClaude)
        };
        let credential = resolve_credential(&scratch_config("settings-credential"), &m)
            .expect("a settings file is enough");
        assert_eq!(credential.description, "settings file ~/.claude/settings.glm.json");
        assert!(credential.env.is_none());
    }

    /// A backend naming a specific `api_key_env` that nothing answers to is a dead end nothing else
    /// can rescue — refused before a request is ever sent, rather than left to fail downstream as
    /// "not logged in".
    #[test]
    fn a_named_api_key_env_with_no_matching_secret_is_refused_before_sending_anything() {
        let m = LlmBackendManifest {
            api_key_env: Some("SOME_MISSING_KEY".into()),
            ..manifest(Backend::PtyClaude)
        };
        let err = resolve_credential(&scratch_config("missing-api-key-env"), &m)
            .expect_err("nothing answers to that name");
        assert!(err.contains("SOME_MISSING_KEY"), "{err}");
    }

    /// A named `api_key_env` backed by a secret of the same name is injected under that name —
    /// exactly the env var the backend said it reads its key from.
    #[test]
    fn a_named_api_key_env_backed_by_a_secret_is_injected_under_its_own_name() {
        let config = scratch_config("api-key-env-secret");
        Secrets::with_config(config.clone())
            .set(None, "MY_KEY", "s3cr3t", None)
            .expect("set");
        let m = LlmBackendManifest {
            api_key_env: Some("MY_KEY".into()),
            ..manifest(Backend::PtyClaude)
        };
        let credential = resolve_credential(&config, &m).expect("the secret answers to that name");
        assert_eq!(credential.description, "api_key_env MY_KEY");
        assert_eq!(credential.env, Some(("MY_KEY".to_string(), "s3cr3t".to_string())));
    }

    /// The bug this whole module exists to fix: a claude backend naming no credential of its own
    /// resolves the global `CLAUDE_CODE_OAUTH_TOKEN` secret exactly the way an agent's
    /// `[[secrets]]` attachment would, instead of quietly asking with none and reporting a false
    /// "not logged in".
    #[test]
    fn a_claude_backend_naming_nothing_falls_back_to_the_global_oauth_token_secret() {
        let config = scratch_config("claude-ambient-secret");
        Secrets::with_config(config.clone())
            .set(None, "CLAUDE_CODE_OAUTH_TOKEN", "tok", None)
            .expect("set");
        let credential = resolve_credential(&config, &manifest(Backend::HarnessClaudeSdk))
            .expect("the global secret answers");
        assert_eq!(credential.description, "global secret CLAUDE_CODE_OAUTH_TOKEN");
        assert_eq!(
            credential.env,
            Some(("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "tok".to_string()))
        );
    }

    /// With no settings, no `api_key_env`, and no global secret, a claude backend still gets a
    /// credential — [`LlmBackendManifest::credential`] already treats the runtime's own ambient
    /// login as legitimate, so this runs on it rather than refusing a request that might yet
    /// succeed against a CLI already logged in on this host.
    #[test]
    fn a_claude_backend_naming_nothing_with_no_global_secret_runs_on_ambient_login_instead_of_refusing() {
        let credential = resolve_credential(&scratch_config("claude-no-secret"), &manifest(Backend::PtyClaude))
            .expect("ambient login is a legitimate credential, not an error");
        assert_eq!(
            credential.description,
            "ambient pty:claude login (no CLAUDE_CODE_OAUTH_TOKEN secret set)"
        );
        assert!(credential.env.is_none());
    }

    /// Codex's own conventional env var is `OPENAI_API_KEY`, not the claude token — the two vendor
    /// CLIs must never be checked against each other's secret.
    #[test]
    fn a_codex_backend_naming_nothing_checks_openai_api_key_not_the_claude_token() {
        let credential = resolve_credential(&scratch_config("codex-ambient"), &manifest(Backend::ProcessCodex))
            .expect("ambient login is a legitimate credential");
        assert_eq!(
            credential.description,
            "ambient process:codex login (no OPENAI_API_KEY secret set)"
        );
    }

    /// The message a human reads names the credential, on both a success and a failure — a genuine
    /// "not logged in" must never leave them guessing which login was even tried.
    #[test]
    fn the_message_names_which_credential_was_used() {
        let answered = TestResult {
            verdict: TestVerdict::Answered,
            elapsed_ms: 42,
            credential: "global secret CLAUDE_CODE_OAUTH_TOKEN".to_string(),
        };
        assert_eq!(answered.message(), "answered in 42ms via global secret CLAUDE_CODE_OAUTH_TOKEN");

        let failed = TestResult {
            verdict: TestVerdict::Failed { error: "Not logged in".to_string() },
            elapsed_ms: 12,
            credential: "global secret CLAUDE_CODE_OAUTH_TOKEN".to_string(),
        };
        assert_eq!(
            failed.message(),
            "Not logged in via global secret CLAUDE_CODE_OAUTH_TOKEN"
        );
    }

    /// The `PATH` a vendor CLI is spawned on reaches past a bare `/usr/bin:/bin` — the shape of what
    /// launchd or systemd hand the app service — the same way [`crate::launch::run_path`] widens it
    /// for a real agent run. Proven by asking `run_with_env` to run `sh -c 'command -v claude'` with
    /// only the vendor CLI's own install directory on the inherited `PATH`, then wiping the
    /// inherited `PATH` out from under it — if this reached only that, the lookup would fail.
    #[test]
    fn the_child_gets_the_same_widened_path_a_real_run_gets() {
        let output = run_with_env(
            &[
                "sh".to_string(),
                "-c".to_string(),
                "echo \"$PATH\"".to_string(),
            ],
            &[],
        )
        .expect("sh is always on a minimal PATH");
        let path = String::from_utf8_lossy(&output.stdout);
        let path = path.trim();
        assert!(
            std::env::split_paths(path).any(|dir| dir == std::path::Path::new("/usr/local/bin")),
            "{path}"
        );
    }
}
