//! The `llm` command group: the backend registry, the shared holds, and the prober.
//!
//! A **backend** is one complete way to answer a turn — a login, a model, the dials that model runs
//! with, how it says "I am out", and how to ask whether it is back. Agents do not carry model
//! settings any more; they carry an ordered list of these, and `agents save --backend <id>` builds
//! that list. So this group is where a model gets configured, and `agents` is where it gets *used*.
//!
//! Two of these verbs exist mainly for the machine rather than the person: `probe --watch` is what
//! the supervised prober service runs, and `holds` is what somebody reads when an agent has moved
//! down its chain and they want to know why.

use adi_core::Adi;
use adi_core::llm::{
    Hold, HoldKey, Holds, LimitRule, LlmBackend, LlmBackendManifest, LlmBackends, LlmSettings,
    Probe, Prober, Verdict, holds::clock, migrate,
};
use clap::Subcommand;

use crate::format::{clean, parse_arguments, print_json};

// `Save` carries the whole backend's worth of flags, dwarfing the id-only variants — a whole-object
// save, deliberately, because a backend is a handful of fields and an omit-to-keep save of a
// partly-remembered one is how a dial goes missing. A one-shot CLI enum, so the size gap costs
// nothing worth boxing over.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub(crate) enum LlmCommand {
    /// List the backends on this machine, with the model each runs, the credential it pays with,
    /// and whether anything is holding it right now.
    Backends {
        #[arg(long)]
        json: bool,
    },
    /// Show one backend in full — its dials, its limit rules, and its probe.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Create or replace a backend. A save states the whole object: a backend is a handful of
    /// fields, and an omit-to-keep save of a partly-remembered one is how a dial goes missing.
    Save {
        /// The id agents name in their rows. Also the filename, so it is a single path segment.
        id: String,
        /// Which runner answers the turn: `harness:adi`, `harness:claude-sdk`, `pty:claude`,
        /// `process:codex`, …
        #[arg(long)]
        runtime: String,
        /// What a human calls it, shown in the panel.
        #[arg(long)]
        label: Option<String>,
        /// The model this backend runs.
        #[arg(long)]
        model: Option<String>,
        /// How much history it can be handed, in tokens. Used to refuse a switch that would not
        /// fit rather than to truncate one; `0` (the default) means unknown, and an unknown window
        /// never refuses anything.
        #[arg(long = "context-tokens")]
        context_tokens: Option<u64>,
        /// The engine settings JSON holding the credential (`~/.claude/settings.glm.json`).
        #[arg(long)]
        settings: Option<String>,
        /// The provider wire for `harness:adi` — `anthropic`, `openai`, `zai`, `ollama`, …
        #[arg(long)]
        provider: Option<String>,
        /// The API base URL, when it is not the provider's default.
        #[arg(long = "base-url")]
        base_url: Option<String>,
        /// The environment variable the API key is read from.
        #[arg(long = "api-key-env")]
        api_key_env: Option<String>,
        /// Repeatable `key=value` dial — `effort=high`, `max_tokens=8192`. Objects and arrays may
        /// be given as JSON. Credentials and `model` are fields of their own and are refused here.
        #[arg(long = "param")]
        params: Vec<String>,
        /// How this backend says "I am out", as the same JSON array the file and the API take:
        /// `[{"match":"(?i)usage limit","class":"quota","scope":"model","resume":"from_message","fixed":"4h"}]`.
        /// A backend with no rules never reroutes — every failure reads as unknown and stops.
        #[arg(long = "limit-rules")]
        limit_rules: Option<String>,
        /// The model the recovery probe runs on — cheaper than this backend's own, usually.
        /// Without either probe flag the backend is never probed and its holds simply expire.
        #[arg(long = "probe-model")]
        probe_model: Option<String>,
        /// The prompt the recovery probe sends. Default `ok`.
        #[arg(long = "probe-prompt")]
        probe_prompt: Option<String>,
        /// Rename: move this definition off the given id and re-point every agent row that named
        /// it. Without this, a rename would silently un-configure every agent using it.
        #[arg(long = "rename-from")]
        rename_from: Option<String>,
    },
    /// Delete a backend. Refused while an agent still lists it — a chain skips a row it cannot
    /// resolve, so deleting out from under one takes an agent's model away and says nothing.
    Delete { id: String },
    /// Read or write the two global switches: `ask_on_switch` and `probe_every`.
    Settings {
        /// Never move a conversation on its own — offer the move and wait to be told. Off by
        /// default, which is what makes the whole chain worth having.
        #[arg(long = "ask-on-switch", value_name = "BOOL")]
        ask_on_switch: Option<bool>,
        /// How often the prober sweeps, in seconds.
        #[arg(long = "probe-every", value_name = "SECONDS")]
        probe_every: Option<u64>,
        #[arg(long)]
        json: bool,
    },
    /// What is held right now, and until when. This is the shared record — one run discovers a
    /// limit and every other run on that credential skips it without spending a turn.
    Holds {
        #[arg(long)]
        json: bool,
    },
    /// Lift a backend's hold by hand, both model-scoped and login-scoped.
    ///
    /// The manual counterpart of the prober, for when you know the limit has lifted — you paid, or
    /// the provider's own dashboard says so — and would rather not wait out a default guess.
    Release { id: String },
    /// Lift every agent's own model configuration out into a backend, and put that backend at the
    /// head of the agent's list. The one-time upgrade; safe to run again.
    ///
    /// Prints the plan and writes nothing unless `--apply` is given, because it rewrites every
    /// agent definition in the store and a plan is the only chance to disagree with it. The
    /// conversion is 1:1 — one backend per distinct configuration found, no merging of agents that
    /// a person would call the same agent. That collapsing is a judgement, and it is done by hand
    /// afterwards with `agents save --llm`.
    Migrate {
        /// Actually write it. Without this the command is a description.
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
    /// Ask held backends whether they are back. This is what must never happen on a chat turn.
    ///
    /// With no id it sweeps every hold whose time is up. With one it probes that backend whatever
    /// its hold says, and writes nothing — a diagnostic for a backend you have just configured.
    Probe {
        /// One backend to probe, instead of sweeping the due holds.
        id: Option<String>,
        /// Sweep forever, every `probe_every` seconds. What the supervised prober service runs.
        #[arg(long, conflicts_with = "id")]
        watch: bool,
        #[arg(long)]
        json: bool,
    },
}

/// Dispatch an `llm` subcommand over the backend registry and the shared hold store.
#[allow(clippy::too_many_lines)] // one arm per verb; splitting it would only move the match
pub(crate) fn run_llm(adi: Adi, command: LlmCommand) -> Result<(), String> {
    let config = adi.agents().config().clone();
    let registry = LlmBackends::with_config(config.clone());
    match command {
        LlmCommand::Backends { json } => {
            let backends = registry.list().map_err(|e| e.to_string())?;
            let holds = Holds::with_config(&config);
            if json {
                let items: Vec<_> = backends
                    .iter()
                    .map(|backend| {
                        serde_json::json!({
                            "id": backend.id,
                            "label": backend.manifest.label,
                            "runtime": backend.manifest.runtime.to_string(),
                            "model": backend.manifest.model,
                            "credential": backend.manifest.credential(),
                            "context_tokens": backend.manifest.context_tokens,
                            "hold": live_hold(&holds, &backend.manifest),
                        })
                    })
                    .collect();
                print_json(&items);
            } else if backends.is_empty() {
                println!("No backends yet. Create one with `adi-mono llm save <id> --runtime …`.");
            } else {
                for backend in &backends {
                    let held = live_hold(&holds, &backend.manifest)
                        .map_or_else(String::new, |hold| format!("  [{}]", hold.describe()));
                    println!(
                        "{} — {} on {}{held}",
                        backend.id,
                        blank(&backend.manifest.model, "the runtime's default model"),
                        backend.manifest.runtime,
                    );
                }
            }
        }
        LlmCommand::Show { id, json } => {
            let backend = require(&registry, &id)?;
            if json {
                print_json(&backend);
            } else {
                print_backend(&backend, &Holds::with_config(&config));
            }
        }
        LlmCommand::Save {
            id,
            runtime,
            label,
            model,
            context_tokens,
            settings,
            provider,
            base_url,
            api_key_env,
            params,
            limit_rules,
            probe_model,
            probe_prompt,
            rename_from,
        } => {
            let from = clean(rename_from).filter(|from| *from != id.trim());
            if let Some(from) = &from {
                require(&registry, from)?;
            }
            let manifest = LlmBackendManifest {
                label: clean(label).unwrap_or_default(),
                runtime: runtime.trim().into(),
                model: clean(model).unwrap_or_default(),
                context_tokens: context_tokens.unwrap_or_default(),
                settings: clean(settings),
                provider: clean(provider),
                base_url: clean(base_url),
                api_key_env: clean(api_key_env),
                params: parse_arguments(params)?,
                limit_rules: parse_limit_rules(limit_rules.as_deref())?,
                probe: probe(probe_model, probe_prompt),
                // The store owns the timestamps.
                created_at: 0,
                updated_at: 0,
            };
            let saved = registry
                .save(id.trim(), manifest)
                .map_err(|e| e.to_string())?;
            if let Some(from) = from {
                let moved = repoint_rows(adi, &from, &saved.id)?;
                registry.delete(&from).map_err(|e| e.to_string())?;
                println!(
                    "Renamed {from} to {}, and re-pointed {moved} agent row{}.",
                    saved.id,
                    if moved == 1 { "" } else { "s" },
                );
            }
            println!(
                "Saved backend {} — {} on {}.",
                saved.id,
                blank(&saved.manifest.model, "the runtime's default model"),
                saved.manifest.runtime,
            );
            if saved.manifest.limit_rules.is_empty() {
                println!(
                    "It has no limit rules, so it will never reroute: every failure on it reads as \
                     unknown and stops the conversation."
                );
            }
        }
        LlmCommand::Delete { id } => {
            let id = id.trim();
            let users = users_of(adi, id)?;
            if !users.is_empty() {
                return Err(format!(
                    "{id} is still listed by {} — take it off {} first",
                    users.join(", "),
                    if users.len() == 1 {
                        "that agent"
                    } else {
                        "those agents"
                    },
                ));
            }
            if registry.delete(id).map_err(|e| e.to_string())? {
                println!("Deleted backend {id}.");
            } else {
                return Err(format!("no backend named {id}"));
            }
        }
        LlmCommand::Settings {
            ask_on_switch,
            probe_every,
            json,
        } => {
            let mut settings = LlmSettings::open(&config);
            if ask_on_switch.is_some() || probe_every.is_some() {
                settings = LlmSettings {
                    ask_on_switch: ask_on_switch.unwrap_or(settings.ask_on_switch),
                    probe_every: probe_every.unwrap_or(settings.probe_every),
                };
                settings
                    .save(&config.module(adi_core::llm::LLM_MODULE))
                    .map_err(|e| e.to_string())?;
            }
            if json {
                print_json(&settings);
            } else {
                println!("ask_on_switch   {}", settings.ask_on_switch);
                println!("probe_every     {}s", settings.probe_every);
            }
        }
        LlmCommand::Holds { json } => {
            let held = Holds::with_config(&config).all().map_err(|e| e.to_string())?;
            if json {
                print_json(&held);
            } else if held.is_empty() {
                println!("Nothing is held — every backend is available.");
            } else {
                for hold in &held {
                    print_hold(hold);
                }
            }
        }
        LlmCommand::Release { id } => {
            let backend = require(&registry, &id)?;
            let credential = backend.manifest.credential();
            let holds = Holds::with_config(&config);
            let mut lifted = 0;
            // Both keys: an `auth` failure was recorded login-wide, and releasing only the
            // model-scoped one would leave the backend just as blocked.
            for key in [
                HoldKey::new(credential.clone(), backend.manifest.model.clone()),
                HoldKey::login(credential),
            ] {
                if holds.release(&key).map_err(|e| e.to_string())? {
                    lifted += 1;
                }
            }
            if lifted == 0 {
                println!("{} was not held.", backend.id);
            } else {
                println!("Released {}.", backend.id);
            }
        }
        LlmCommand::Migrate { apply, json } => {
            let agents = adi.agents();
            let plan = migrate::plan(&agents, &registry).map_err(|e| e.to_string())?;
            if apply {
                let changed =
                    migrate::apply(&agents, &registry, &plan).map_err(|e| e.to_string())?;
                report_migration(&plan, json, Some(changed));
            } else {
                report_migration(&plan, json, None);
            }
        }
        LlmCommand::Probe { id, watch, json } => {
            let prober = Prober::with_config(config.clone());
            if let Some(id) = id {
                let verdict = prober.probe_backend(id.trim()).map_err(|e| e.to_string())?;
                if json {
                    print_json(&serde_json::json!({ "backend": id.trim(), "verdict": describe(&verdict) }));
                } else {
                    println!("{} — {}", id.trim(), describe(&verdict));
                }
                return Ok(());
            }
            if watch {
                return watch_forever(&prober, json);
            }
            let swept = prober.sweep().map_err(|e| e.to_string())?;
            report_sweep(&swept, json);
        }
    }
    Ok(())
}

/// How often a switched-off watcher looks again. Nothing is probed while `probe_every` is `0`, but
/// the process stays up and keeps reading the setting: somebody who turns probing back on should not
/// have to remember to restart this too.
const OFF_POLL: std::time::Duration = std::time::Duration::from_secs(30);

/// Sweep on a timer until killed — the hand-run form of the worker `adi-app` runs for the panel
/// (`crates/adi-app/src/prober.rs`), for a store the app is not watching.
///
/// The interval is re-read from the settings file on every pass rather than captured at startup, so
/// changing `probe_every` takes effect within one cycle instead of at the next restart — and
/// `probe_every = 0` stops the sweeping without stopping the process. Nothing here exits on a store
/// error either: a sweep that could not read the database is a bad minute, not a reason for whatever
/// supervises this to start counting restarts.
fn watch_forever(prober: &Prober, json: bool) -> Result<(), String> {
    loop {
        let Some(every) = prober.interval() else {
            std::thread::sleep(OFF_POLL);
            continue;
        };
        match prober.sweep() {
            Ok(swept) => report_sweep(&swept, json),
            Err(e) => eprintln!("sweep failed: {e}"),
        }
        std::thread::sleep(every);
    }
}

/// Print a migration plan — or, when `changed` is set, what carrying it out did.
///
/// It reads as a plan either way, on purpose: the same lines somebody approved are the lines that
/// say what happened, so there is nothing to compare between two differently-shaped outputs.
fn report_migration(plan: &migrate::Plan, json: bool, changed: Option<usize>) {
    if json {
        print_json(&serde_json::json!({
            "applied": changed.is_some(),
            "changed": changed,
            "backends": plan.backends.keys().collect::<Vec<_>>(),
            "moves": plan.moves.iter().map(|mv| serde_json::json!({
                "agent": mv.agent,
                "backend": mv.backend,
                "created": mv.created,
                "moved": mv.moved,
            })).collect::<Vec<_>>(),
            "skipped": plan.skipped.iter().map(|skip| serde_json::json!({
                "agent": skip.agent,
                "why": skip.why,
            })).collect::<Vec<_>>(),
        }));
        return;
    }
    if plan.is_empty() {
        println!("Nothing to migrate — every agent already lists its backends.");
        return;
    }
    if !plan.backends.is_empty() {
        println!(
            "{} new backend{}:",
            plan.backends.len(),
            if plan.backends.len() == 1 { "" } else { "s" }
        );
        for (id, manifest) in &plan.backends {
            let model = if manifest.model.is_empty() {
                "the runtime's own model".to_string()
            } else {
                manifest.model.clone()
            };
            println!("  {id:<20} {} · {model} · {}", manifest.runtime, manifest.credential());
        }
        println!();
    }
    println!(
        "{} agent{}:",
        plan.moves.len(),
        if plan.moves.len() == 1 { "" } else { "s" }
    );
    for mv in &plan.moves {
        let moved = if mv.moved.is_empty() {
            "nothing to strip".to_string()
        } else {
            format!("moves {}", mv.moved.join(", "))
        };
        let mark = if mv.created { "new" } else { "reuses" };
        println!("  {:<20} -> {} ({mark}), {moved}", mv.agent, mv.backend);
    }
    if !plan.skipped.is_empty() {
        println!("\nleft alone:");
        for skip in &plan.skipped {
            println!("  {:<20} {}", skip.agent, skip.why);
        }
    }
    match changed {
        Some(changed) => println!("\nMigrated {changed}."),
        // The whole point of the dry run is that it is one; say so where it will be read.
        None => println!("\nNothing written. Re-run with --apply to do it."),
    }
}

fn report_sweep(swept: &[adi_core::llm::Checked], json: bool) {
    if json {
        let items: Vec<_> = swept
            .iter()
            .map(|checked| {
                serde_json::json!({
                    "credential": checked.key.credential,
                    "model": checked.key.model,
                    "backend": checked.backend,
                    "verdict": describe(&checked.verdict),
                })
            })
            .collect();
        print_json(&items);
        return;
    }
    if swept.is_empty() {
        // Silent in `--watch`, where this is every line: a prober that logs "nothing due" once a
        // minute forever buries the one line that matters.
        return;
    }
    for checked in swept {
        println!(
            "{} — {}",
            checked.backend.as_deref().unwrap_or(&checked.key.credential),
            describe(&checked.verdict),
        );
    }
}

/// One verdict as the sentence somebody reads in a log. The provider's own words are carried
/// through verbatim: they are the evidence for a hold nobody else can check.
fn describe(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Back => "back — the hold is released".to_string(),
        Verdict::StillOut { until, reason } => {
            format!("still limited until {} ({reason})", clock(*until))
        }
        Verdict::Failed { error } => format!("failed, and not with a limit: {error}"),
        Verdict::Unreachable { reason } => format!("not probed — {reason}"),
    }
}

/// The hold blocking a backend right now, if any. A store failure reads as "nothing known to be
/// held", which is what an empty store means too — the listing is still worth printing.
fn live_hold(holds: &Holds, manifest: &LlmBackendManifest) -> Option<Hold> {
    holds
        .blocking(&HoldKey::new(manifest.credential(), manifest.model.clone()))
        .ok()
        .flatten()
}

fn print_backend(backend: &LlmBackend, holds: &Holds) {
    let m = &backend.manifest;
    println!("{} — {}", backend.id, blank(&m.label, "(no label)"));
    println!("runtime         {}", m.runtime);
    println!("model           {}", blank(&m.model, "(the runtime's own)"));
    println!("credential      {}", m.credential());
    if m.context_tokens > 0 {
        println!("context         {} tokens", m.context_tokens);
    }
    for (key, value) in &m.params {
        println!("param           {key} = {value}");
    }
    for rule in &m.limit_rules {
        println!(
            "limit rule      {:?} → {:?}, {:?}-scoped",
            rule.pattern, rule.class, rule.scope
        );
    }
    match &m.probe {
        Some(probe) => println!(
            "probe           {:?} on {}",
            probe.prompt,
            probe.model.as_deref().unwrap_or("its own model"),
        ),
        None => println!("probe           none — a hold on it expires rather than being checked"),
    }
    match live_hold(holds, m) {
        Some(hold) => print_hold(&hold),
        None => println!("status          available"),
    }
}

fn print_hold(hold: &Hold) {
    println!(
        "held            {} {} — {:?}, {} (found by {}, {} probe{})",
        hold.key.credential,
        blank(&hold.key.model, "(the whole login)"),
        hold.class,
        hold.describe(),
        hold.set_by,
        hold.attempts,
        if hold.attempts == 1 { "" } else { "s" },
    );
    if !hold.reason.is_empty() {
        println!("                {}", hold.reason);
    }
}

fn require(registry: &LlmBackends, id: &str) -> Result<LlmBackend, String> {
    registry
        .get(id.trim())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no backend named {}", id.trim()))
}

/// Which agents list this backend. Read straight off the definitions, because a backend that knew
/// its own users would be a login record and this design has none.
fn users_of(adi: Adi, id: &str) -> Result<Vec<String>, String> {
    Ok(adi
        .agents()
        .list()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|agent| agent.manifest.backends.iter().any(|row| row.backend == id))
        .map(|agent| agent.name)
        .collect())
}

/// Re-point every agent row naming `from` at `to`, through the ordinary save so the rows are
/// validated and `adi.agents.saved` fires — a rename is an edit of those agents.
fn repoint_rows(adi: Adi, from: &str, to: &str) -> Result<usize, String> {
    let store = adi.agents();
    let mut moved = 0;
    for agent in store.list().map_err(|e| e.to_string())? {
        if !agent.manifest.backends.iter().any(|row| row.backend == from) {
            continue;
        }
        let mut manifest = agent.manifest;
        for row in &mut manifest.backends {
            if row.backend == from {
                row.backend = to.to_string();
            }
        }
        store.save(&agent.name, manifest).map_err(|e| e.to_string())?;
        moved += 1;
    }
    Ok(moved)
}

/// Read `--limit-rules` as the same JSON array the file and the API take.
///
/// One flag carrying the whole list rather than a repeatable mini-language: a rule has five fields
/// and its `match` is a regular expression, so any separator worth typing is a character a regex
/// wants. The shape here is exactly the shape in the TOML, which means the panel, the API and this
/// are one thing to learn instead of three.
fn parse_limit_rules(value: Option<&str>) -> Result<Vec<LimitRule>, String> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(Vec::new());
    };
    serde_json::from_str(value).map_err(|e| {
        format!(
            "--limit-rules is not a JSON array of rules: {e}\n  \
             expected [{{\"match\": \"(?i)usage limit\", \"class\": \"quota\", \
             \"scope\": \"model\", \"resume\": \"from_message\", \"fixed\": \"4h\"}}]"
        )
    })
}

/// Build the probe from its two flags. Neither given means no probe at all — and that is the
/// difference between a backend that is checked and one whose holds simply expire, so it is stated
/// by the operator rather than defaulted into existence.
fn probe(model: Option<String>, prompt: Option<String>) -> Option<Probe> {
    let model = clean(model);
    let prompt = clean(prompt);
    if model.is_none() && prompt.is_none() {
        return None;
    }
    Some(Probe {
        model,
        prompt: prompt.unwrap_or_else(|| Probe::default().prompt),
    })
}

fn blank<'a>(value: &'a str, instead: &'a str) -> &'a str {
    if value.trim().is_empty() {
        instead
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_rules_read_as_the_json_the_file_and_the_api_use() {
        let rules = parse_limit_rules(Some(
            r#"[{"match":"(?i)usage limit","class":"quota","scope":"model","resume":"from_message","fixed":"4h"}]"#,
        ))
        .expect("parses");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "(?i)usage limit");
        assert_eq!(rules[0].class, adi_core::llm::LimitClass::Quota);
        assert_eq!(rules[0].fixed_seconds(), 4 * 60 * 60);
    }

    /// The error has to carry the shape, not just "invalid JSON": somebody typing this at a shell
    /// has no form to look at.
    #[test]
    fn an_unreadable_rule_list_says_what_one_looks_like() {
        let err = parse_limit_rules(Some("quota")).expect_err("refused");
        assert!(err.contains("\"class\": \"quota\""), "{err}");
        assert!(parse_limit_rules(None).expect("none").is_empty());
        assert!(parse_limit_rules(Some("  ")).expect("blank").is_empty());
    }

    /// A probe is a real billed request repeating forever on a timer, so it exists only because
    /// somebody asked for it — never because a default filled it in.
    #[test]
    fn a_probe_appears_only_when_a_flag_asks_for_one() {
        assert!(probe(None, None).is_none());
        assert!(probe(None, Some("  ".into())).is_none());

        let only_model = probe(Some("haiku".into()), None).expect("a probe");
        assert_eq!(only_model.model.as_deref(), Some("haiku"));
        assert_eq!(only_model.prompt, "ok", "the default prompt fills in");

        let only_prompt = probe(None, Some("ping".into())).expect("a probe");
        assert_eq!(only_prompt.model, None, "probed on the backend's own model");
        assert_eq!(only_prompt.prompt, "ping");
    }
}
