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

use std::process::Command;
use std::time::Instant;

use adi_config::{Config, now_unix};

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
}

impl TestResult {
    /// One line for a human — what happened, in the words the provider used where there are any.
    #[must_use]
    pub fn message(&self) -> String {
        match &self.verdict {
            TestVerdict::Answered => format!("answered in {}ms", self.elapsed_ms),
            TestVerdict::RateLimited { reason } => format!("still rate-limited — {reason}"),
            TestVerdict::Failed { error } => error.clone(),
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
    Ok(test_manifest(&backend.manifest))
}

/// Test a backend that may not be saved at all — a draft an operator is still typing into the panel.
/// This is the whole reason the core function takes a manifest rather than an id: the point of a
/// "Test" button beside a form is to test the form as it stands, not whatever was last saved.
#[must_use]
pub fn test_manifest(manifest: &LlmBackendManifest) -> TestResult {
    let (prompt, model) = test_prompt(manifest);

    let start = Instant::now();
    let outcome = ask(manifest, model, prompt);
    let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

    let verdict = match outcome {
        Ok(()) => TestVerdict::Answered,
        Err(text) => {
            let found = classify(&text, &manifest.limit_rules, None, now_unix());
            if found.class.holds() {
                TestVerdict::RateLimited { reason: found.evidence }
            } else {
                TestVerdict::Failed { error: text }
            }
        }
    };
    TestResult { verdict, elapsed_ms }
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
fn ask(manifest: &LlmBackendManifest, model: &str, prompt: &str) -> std::result::Result<(), String> {
    match &manifest.runtime {
        Backend::HarnessAdi => {
            crate::backends::harness::adi_loop::probe(&manifest.arguments(), model, prompt)
                .map(drop)
                .map_err(|e| e.to_string())
        }
        // All three run the `claude` CLI; a pty backend's live session and a harness backend's
        // scoped turn are both beside the point here, so every one of them gets the same plain
        // `--print` ask instead.
        Backend::PtyClaude | Backend::ProcessClaude | Backend::HarnessClaudeSdk => {
            let argv = claude_argv(model, manifest.settings.as_deref(), prompt);
            run(&argv).and_then(|output| {
                if output.status.success() {
                    Ok(())
                } else {
                    Err(command_error("claude", &output))
                }
            })
        }
        // Likewise for `codex`: `codex exec` is the headless one-shot both the pty and the process
        // engine ultimately open a terminal or a child around.
        Backend::PtyCodex | Backend::ProcessCodex => {
            let argv = codex_argv(model, prompt);
            let mut env = Vec::new();
            crate::backends::quiet_codex_env(&mut env);
            run_with_env(&argv, &env).and_then(|output| read_codex(&output))
        }
        Backend::Other(other) => Err(format!(
            "{} is not a runtime this build knows how to test",
            if other.is_empty() { "(no runtime set)" } else { other }
        )),
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

fn run(argv: &[String]) -> std::result::Result<std::process::Output, String> {
    run_with_env(argv, &[])
}

fn run_with_env(
    argv: &[String],
    env: &[(String, String)],
) -> std::result::Result<std::process::Output, String> {
    let (program, rest) = argv.split_first().expect("argv always starts with the program");
    let mut cmd = Command::new(program);
    cmd.args(rest).envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
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

    /// The one refusal that remains: a backend naming no runtime at all, or one this build has never
    /// heard of, has nothing to spawn. This is a pure dispatch failure — it never reaches `Command`,
    /// which is what makes it safe to run as an ordinary unit test.
    #[test]
    fn an_unset_runtime_fails_by_name_rather_than_being_asked_anything() {
        let result = test_manifest(&manifest(Backend::default()));
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
}
