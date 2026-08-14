//! The `agents` command group: the agent-definition subcommand surface and its dispatch
//! over the shared agent-definition store.

use std::collections::BTreeMap;

use adi_core::{
    Adi, AgentManifest, Agents, AgentSummaryArguments, AgentsError, Launch, RunInfo,
    SecretAttachment, StoredAgent,
};
use clap::Subcommand;

use crate::format::{clean, clean_required, clean_tags, parse_arguments, print_json};

// `Save` carries the whole definition's worth of flags, dwarfing the name-only variants; a
// one-shot CLI enum, so the size gap costs nothing worth boxing over.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub(crate) enum AgentsCommand {
    /// List agent definitions.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one agent definition.
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Create or replace an agent definition.
    Save {
        name: String,
        /// The `executor:what` backend, e.g. `pty:claude`, `process:codex`,
        /// `harness:claude-sdk`, `harness:adi`.
        #[arg(long)]
        backend: String,
        #[arg(long)]
        system_prompt: Option<String>,
        /// CLI command groups this agent may use, stored as the manifest's command scope.
        #[arg(long = "command-scope")]
        command_scope: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        permission_mode: Option<String>,
        #[arg(long)]
        temperature: Option<f64>,
        #[arg(long)]
        max_turns: Option<u32>,
        /// Repeatable; comma-separated values are also accepted. Omit every `--tag` to leave an
        /// existing agent's tags alone; pass `--no-tag` to clear them.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Clear the agent's tags (see `--tag`).
        #[arg(long, conflicts_with = "tags")]
        no_tag: bool,
        /// Star the agent. Omit both this and `--no-starred` to leave the setting as it was.
        #[arg(long)]
        starred: bool,
        /// Take the agent's star away (see `--starred`).
        #[arg(long, conflicts_with = "starred")]
        no_starred: bool,
        /// The project to file the agent under (its id). Omit it to leave an existing agent's
        /// project alone; pass `--no-project` to make it global.
        #[arg(long)]
        project: Option<String>,
        /// Unfile the agent from its project, making it global (see `--project`).
        #[arg(long, conflicts_with = "project")]
        no_project: bool,
        /// An adi tool id to enable for this agent (its own `.bin`). Repeatable; comma-separated
        /// values are also accepted. Omit every `--tool` to leave an existing agent's tools as they
        /// are; pass `--no-tool` to take them all away. Distinct from `--command-scope` (the LLM's
        /// allowed tools).
        #[arg(long = "tool")]
        tools: Vec<String>,
        /// Clear the agent's enabled tools (see `--tool`).
        #[arg(long, conflicts_with = "tools")]
        no_tool: bool,
        /// A secret to attach to this agent (injected into its runs as an env var under its name).
        /// Give `NAME` for a global secret or `PROJECT/NAME` for a project-scoped one. Repeatable;
        /// comma-separated values are also accepted. Only attached secrets are injected — an
        /// explicit allowlist. Omit every `--secret` to leave an existing agent's attachments
        /// alone; pass `--no-secret` to detach them all.
        #[arg(long = "secret")]
        secrets: Vec<String>,
        /// Detach every secret from the agent (see `--secret`).
        #[arg(long, conflicts_with = "secrets")]
        no_secret: bool,
        /// A directory to put on this agent's run `PATH`, ahead of the machine's own — how an agent
        /// pins a toolchain the default `PATH` doesn't point at. `~` and `$HOME` are expanded at
        /// launch. Repeatable. Omit every `--path` to leave an existing agent's dirs as they are;
        /// pass `--no-path` to clear them.
        #[arg(long = "path")]
        path: Vec<String>,
        /// Clear the agent's extra `PATH` dirs (see `--path`).
        #[arg(long, conflicts_with = "path")]
        no_path: bool,
        /// An environment variable for this agent's runs, as `KEY=VALUE`. Repeatable. Omit every
        /// `--env` to leave an existing agent's variables alone; pass `--no-env` to clear them.
        /// `PATH` is rejected here — it is built from `--path`.
        #[arg(long = "env")]
        env: Vec<String>,
        /// Clear the agent's extra environment variables (see `--env`).
        #[arg(long, conflicts_with = "env")]
        no_env: bool,
        /// This agent runs with nobody watching, so it may not stop to ask: the `Ask` tool refuses
        /// and tells the run to decide for itself and say what it assumed. For a trigger's agent, a
        /// scheduled sweep — anything whose questions nobody would see. Omit both this and
        /// `--no-unattended` to leave the setting as it was.
        #[arg(long)]
        unattended: bool,
        /// Let the agent stop and ask again (see `--unattended`).
        #[arg(long, conflicts_with = "unattended")]
        no_unattended: bool,
        /// A knowledge base this agent works with — `global/<name>`, `project:<id>/<name>`, or
        /// `agent:<name>/<base>`. Repeatable; comma-separated values are also accepted. Omit every
        /// `--knowledge` to leave an existing agent's bases alone; pass `--no-knowledge` to clear
        /// them. What the agent may actually reach is still decided by the isolation levels.
        #[arg(long = "knowledge")]
        knowledge: Vec<String>,
        /// Clear the agent's knowledge bases (see `--knowledge`).
        #[arg(long, conflicts_with = "knowledge")]
        no_knowledge: bool,
        /// Give this agent a memory of its own: the `agent:<name>/memory` base, which it alone
        /// writes and every other agent may read. Omit both this and `--no-memory` to leave the
        /// setting as it was, so a save that never mentions memory cannot quietly take an
        /// agent's away.
        #[arg(long)]
        memory: bool,
        /// Take the agent's own memory away (see `--memory`).
        #[arg(long, conflicts_with = "memory")]
        no_memory: bool,
        /// Repeatable key=value backend argument. Objects and arrays may be supplied as JSON.
        /// Overlaid on the agent's existing arguments — what you don't state stays as it was.
        /// Pass `--no-argument` to start from nothing instead.
        #[arg(long = "argument", visible_alias = "extra")]
        arguments: Vec<String>,
        /// Discard the agent's existing backend arguments (its system prompt included) and keep
        /// only what this invocation states.
        #[arg(long)]
        no_argument: bool,
        #[arg(long)]
        json: bool,
    },
    /// Launch an agent in its backend. Pty executors open an interactive session and
    /// process executors run a headless CLI in the background.
    Run {
        name: String,
        /// The task sent to a process backend (ignored by pty backends).
        #[arg(short, long, default_value = "run")]
        message: String,
        /// Where this run starts, overriding the agent's own `working_dir` and its project
        /// directory. For pointing one agent at a different target each run.
        #[arg(long, value_name = "PATH")]
        dir: Option<String>,
        /// Launch even when as many runs are already live as the store allows. The deliberate
        /// override of the concurrency limit — for a human who wants this one now.
        #[arg(long)]
        force: bool,
        /// Block until the run finishes, print its answer, and exit with its status.
        ///
        /// What turns a launch into something another program can be composed out of: a shell
        /// script, a CI step, or an agent's own `Bash` — which is how one agent waits for another
        /// without either of them learning a new verb. Not for interactive backends: a pty run has
        /// no ending to wait for, only a pane somebody is looking at.
        #[arg(long)]
        wait: bool,
    },
    /// List runs across every agent (or one), newest first, with what became of each.
    ///
    /// What a supervisor reads instead of opening conversations: a finished run carries its
    /// engine's own verdict, so "which of these died, and how" is one command rather than a hunt
    /// through logs. Listing is also what *notices* an ending, so running this is what publishes
    /// `adi.agents.run.finished` for anything that stopped since the last look.
    Runs {
        /// Only this agent's runs.
        #[arg(long)]
        agent: Option<String>,
        /// `running`, `failed` (the engine called it an error), `done` (finished without one), or
        /// `unknown` (stopped before an outcome was ever recorded).
        #[arg(long)]
        status: Option<String>,
        /// Only runs started within this window: `90s`, `30m`, `2h`, `7d`.
        #[arg(long)]
        since: Option<String>,
        /// At most this many rows, after filtering.
        #[arg(long, default_value_t = 40)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Show, or set, how many agent runs may be live at once — overall, or (with `--project`) for
    /// one project's agents. The caps bind every launch the platform makes on its own (a trigger
    /// firing, a queued chat turn); a human overrides them per run with `agents run --force`.
    Limit {
        /// The new cap. `0` lifts it (for a project: clears its own cap). Omit to read the
        /// current ones.
        max: Option<u32>,
        /// Set this project's cap instead of the global one. A project cap narrows the global
        /// number, it never lifts it.
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List the questions runs are waiting on a person to answer.
    ///
    /// A run that needs a decision writes it down and ends its turn, so nothing is held open —
    /// which also means nothing will move until somebody answers. This is what is waiting.
    Questions {
        /// Only this agent's questions.
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Answer the question a conversation is waiting on, and let it carry on.
    ///
    /// Give one reply per question, in the order they were asked (`agents questions` prints them
    /// numbered). One reply for a single-question ask is the ordinary case.
    Answer {
        /// The agent whose conversation is waiting.
        name: String,
        /// The conversation id — what `agents questions` prints as `conv`.
        conv: String,
        /// The answers, in the order the questions were asked.
        #[arg(required = true)]
        replies: Vec<String>,
        /// Answer this specific ask, refusing if it has been settled since. Without it, whatever is
        /// pending now is answered.
        #[arg(long)]
        ask: Option<String>,
    },
    /// Stop a running agent using its executor's lifecycle.
    Stop { name: String },
    /// Delete an agent definition.
    Rm { name: String },
    /// Delete an agent definition.
    Delete { name: String },
}

/// Dispatch an `agents` subcommand over the shared agent-definition store.
pub(crate) fn run_agents(adi: Adi, command: AgentsCommand) -> Result<(), String> {
    let store = adi.agents();
    match command {
        AgentsCommand::List { json } => {
            let agents = store.list().map_err(|e| e.to_string())?;
            if json {
                print_json(&agents);
            } else if agents.is_empty() {
                println!("No agents registered.");
            } else {
                for agent in &agents {
                    print_agent(agent);
                }
            }
        }
        AgentsCommand::Show { name, json } => {
            let agent = store
                .get(&name)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such agent: {name}"))?;
            if json {
                print_json(&agent);
            } else {
                print_agent(&agent);
            }
        }
        AgentsCommand::Save {
            name,
            backend,
            system_prompt,
            command_scope,
            model,
            permission_mode,
            temperature,
            max_turns,
            tags,
            no_tag,
            starred,
            no_starred,
            project,
            no_project,
            tools,
            no_tool,
            secrets,
            no_secret,
            path,
            no_path,
            env,
            no_env,
            unattended,
            no_unattended,
            knowledge,
            no_knowledge,
            memory,
            no_memory,
            arguments: arguments_flags,
            no_argument,
            json,
        } => {
            let backend = clean_required("backend", backend)?;
            // Read first: everything below either states a field or leaves the stored one alone,
            // and both need what is already there.
            let stored = store.get(&name).ok().flatten().map(|a| a.manifest);
            // The engine's arguments are *overlaid* on what the agent already had, not swapped for
            // them. They are one map holding unrelated things — the system prompt beside the model
            // beside a max-turns cap — so a save that states the model and says nothing else means
            // "set the model", never "and throw the prompt away". It used to mean the latter, which
            // made `agents save --tool …` on a live agent a way to silently erase its instructions.
            // `--no-argument` is how the map is actually emptied.
            let mut arguments = if no_argument {
                BTreeMap::new()
            } else {
                stored
                    .as_ref()
                    .map(|m| m.arguments.clone())
                    .unwrap_or_default()
            };
            arguments.extend(parse_arguments(arguments_flags)?);
            if let Some(value) = clean(system_prompt) {
                arguments.insert("system_prompt".into(), value.into());
            }
            if let Some(value) = clean(command_scope) {
                arguments.insert("tools".into(), value.into());
            }
            if let Some(value) = clean(model) {
                arguments.insert("model".into(), value.into());
            }
            if let Some(value) = clean(permission_mode) {
                arguments.insert("permission_mode".into(), value.into());
            }
            if let Some(value) = temperature {
                arguments.insert("temperature".into(), value.into());
            }
            if let Some(value) = max_turns {
                arguments.insert("max_turns".into(), value.into());
            }
            // Everything below follows the same rule as the arguments above: stated wins, omitted
            // keeps, and each field has an explicit `--no-…` for actually taking it away.
            //
            // `tags`, `starred`, `project`, `secrets`, and `unattended` did not, and the comment
            // above claimed they did. A save stating one field replaced all five with whatever the
            // flags happened to default to — so `agents save <name> --backend … --memory`, the
            // obvious way to turn a setting on, silently unfiled the agent from its project,
            // dropped its tags and its star, and detached every secret. It cost 62 agents at once
            // to find. A field that resets itself when unmentioned is not a default, it is a trap:
            // the caller cannot state what they do not know the flag exists for.
            // Each field is `kept`/`flag`, never a hand-rolled if/else. That uniformity is the
            // actual fix: the five that were wrong were wrong *individually*, each written out
            // longhand at its own call site, and nothing made the odd one out visible.
            let old = stored.as_ref();
            let manifest = AgentManifest {
                backend: backend.into(),
                arguments,
                tags: kept(no_tag, stated(tags, clean_tags), old.map(|m| m.tags.clone())),
                starred: flag(starred, no_starred, old.is_some_and(|m| m.starred)),
                // The costliest of the five to get wrong: an agent's project decides which
                // database, secrets, and knowledge bases its runs reach, so a save that dropped it
                // did not merely lose a label — it moved the agent somewhere else.
                // Doubly wrapped on purpose: the outer `Option` is "was it mentioned", the inner is
                // the agent's own "filed under a project, or global".
                project: kept(
                    no_project,
                    clean(project).map(Some),
                    old.map(|m| m.project.clone()),
                ),
                bin_tools: kept(no_tool, stated(tools, clean_tags), old.map(|m| m.bin_tools.clone())),
                secrets: kept(
                    no_secret,
                    stated(secrets, parse_secret_attachments),
                    old.map(|m| m.secrets.clone()),
                ),
                // Not `clean_tags`: a directory may legitimately contain a comma, so each `--path`
                // is one dir, never a comma-separated list.
                path: kept(
                    no_path,
                    stated(path, |dirs| {
                        dirs.into_iter()
                            .map(|dir| dir.trim().to_string())
                            .filter(|dir| !dir.is_empty())
                            .collect()
                    }),
                    old.map(|m| m.path.clone()),
                ),
                env: kept(
                    no_env,
                    if env.is_empty() {
                        None
                    } else {
                        Some(parse_env_vars(env)?)
                    },
                    old.map(|m| m.env.clone()),
                ),
                unattended: flag(unattended, no_unattended, old.is_some_and(|m| m.unattended)),
                knowledge: kept(
                    no_knowledge,
                    stated(knowledge, clean_tags),
                    old.map(|m| m.knowledge.clone()),
                ),
                memory: flag(memory, no_memory, old.is_some_and(|m| m.memory)),
                created_at: 0,
                updated_at: 0,
            };
            let agent = store.save(&name, manifest).map_err(|e| e.to_string())?;
            if json {
                print_json(&agent);
            } else {
                println!("Saved agent {}.", agent.name);
                print_agent(&agent);
            }
        }
        AgentsCommand::Run {
            name,
            message,
            dir,
            force,
            wait,
        } => {
            let launch = if force {
                store.force_run_in(&name, &message, dir.as_deref())
            } else {
                store.run_in(&name, &message, dir.as_deref())
            }
            .map_err(|e| match e {
                // The refusal is the one error a human can act on from here, so it says how.
                AgentsError::TooManyRunning { .. } => {
                    format!("{e}: re-run with --force to launch it anyway")
                }
                other => other.to_string(),
            })?;
            match launch {
                Launch::Pty { command, session } => {
                    if wait {
                        return Err(
                            "--wait needs a run that ends; a pty agent opens a pane instead"
                                .to_string(),
                        );
                    }
                    println!("Started agent {name} in session {session}.");
                    println!("  command: {command}");
                    println!("  view:    in the control panel's live view (no external attach)");
                }
                Launch::Process {
                    command,
                    pid,
                    log,
                    run_id,
                } => {
                    if wait {
                        return await_run(&store, &name, &run_id);
                    }
                    println!("Started agent {name} as background process {pid}.");
                    println!("  run:     {run_id}");
                    println!("  command: {command}");
                    println!("  log:     {}", log.display());
                }
            }
        }
        AgentsCommand::Runs {
            agent,
            status,
            since,
            limit,
            json,
        } => {
            let cutoff = match since.as_deref().map(parse_since).transpose()? {
                Some(window) => now_ms().saturating_sub(window),
                None => 0,
            };
            let wanted = status.as_deref().map(str::trim).filter(|s| !s.is_empty());
            let agents = match &agent {
                Some(name) => vec![
                    store
                        .get(name)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("no such agent: {name}"))?,
                ],
                None => store.list().map_err(|e| e.to_string())?,
            };

            let mut rows: Vec<RunRow> = Vec::new();
            for a in &agents {
                for run in store.runs(a) {
                    if run.started_at < cutoff {
                        continue;
                    }
                    let row = RunRow::of(&a.name, run);
                    if wanted.is_some_and(|want| row.status != want) {
                        continue;
                    }
                    rows.push(row);
                }
            }
            // Newest first across every agent, which is the order a supervisor reads in — the
            // per-agent listings arrive already sorted but interleaved.
            rows.sort_unstable_by(|a, b| b.started_at.cmp(&a.started_at));
            rows.truncate(limit);

            if json {
                print_json(&rows);
            } else if rows.is_empty() {
                println!("No runs match.");
            } else {
                for row in &rows {
                    row.print();
                }
            }
        }
        AgentsCommand::Limit {
            max,
            project,
            json,
        } => {
            let limits = match (max, project.as_deref()) {
                (Some(max), Some(project)) => store
                    .set_project_limit(project, max)
                    .map_err(|e| e.to_string())?,
                (Some(max_concurrent_runs), None) => {
                    let mut limits = store.limits();
                    limits.max_concurrent_runs = max_concurrent_runs;
                    store.set_limits(limits).map_err(|e| e.to_string())?
                }
                (None, _) => store.limits(),
            };
            let load = store.run_load();
            if json {
                print_json(&serde_json::json!({
                    "max_concurrent_runs": limits.max_concurrent_runs,
                    "running": load.total(),
                    "projects": limits.projects.iter().map(|(id, max)| serde_json::json!({
                        "project": id,
                        "max_concurrent_runs": max,
                        "running": load.in_project(id),
                    })).collect::<Vec<_>>(),
                }));
            } else {
                if limits.max_concurrent_runs == 0 {
                    println!("No overall limit ({} running).", load.total());
                } else {
                    println!(
                        "At most {} agent runs at once ({} running).",
                        limits.max_concurrent_runs,
                        load.total()
                    );
                }
                for (id, max) in &limits.projects {
                    println!(
                        "  {id}: at most {max} ({} running)",
                        load.in_project(id)
                    );
                }
            }
        }
        AgentsCommand::Questions { agent, json } => {
            let agent = clean(agent);
            let waiting: Vec<_> = store
                .pending_questions()
                .into_iter()
                .filter(|ask| agent.as_ref().is_none_or(|name| ask.agent == *name))
                .collect();
            if json {
                print_json(&waiting);
            } else if waiting.is_empty() {
                println!("Nothing is waiting on you.");
            } else {
                for ask in &waiting {
                    print_ask(ask);
                }
            }
        }
        AgentsCommand::Answer {
            name,
            conv,
            replies,
            ask,
        } => {
            let ask = clean(ask);
            let sent = store
                .answer(&name, &conv, ask.as_deref(), &replies)
                .map_err(|e| e.to_string())?;
            match sent {
                adi_core::Sent::Started(_) => println!("Answered — {name} is working on it."),
                adi_core::Sent::Queued { place } => println!(
                    "Answered — {place} in line behind what {name} is already saying."
                ),
            }
        }
        AgentsCommand::Stop { name } => {
            if store.stop(&name).map_err(|e| e.to_string())? {
                println!("Stopped agent {name}.");
            } else {
                println!("Agent {name} wasn't running.");
            }
        }
        AgentsCommand::Rm { name } | AgentsCommand::Delete { name } => {
            if store.delete(&name).map_err(|e| e.to_string())? {
                println!("Deleted agent {name}.");
            } else {
                println!("No such agent: {name}.");
            }
        }
    }
    Ok(())
}

/// Print one waiting ask: who is blocked, on what, and the numbered questions to answer in order.
///
/// Numbered because `agents answer` takes the replies positionally — the numbers here are the order
/// to type them in, which is the whole reason a multi-question ask is legible from a terminal at all.
fn print_ask(ask: &adi_core::Ask) {
    println!("{} · conv {}", ask.agent, ask.conv);
    println!("  ask {}", ask.id);
    if !ask.note.is_empty() {
        println!("  {}", ask.note);
    }
    for (index, question) in ask.questions.iter().enumerate() {
        let header = if question.header.is_empty() {
            String::new()
        } else {
            format!("[{}] ", question.header)
        };
        println!("  {}. {header}{}", index + 1, question.question);
        for option in &question.options {
            match option.description.is_empty() {
                true => println!("       - {}", option.label),
                false => println!("       - {} — {}", option.label, option.description),
            }
        }
    }
    println!(
        "  answer with: adi-mono agents answer {} {} {}",
        ask.agent,
        ask.conv,
        ask.questions
            .iter()
            .map(|_| "'…'")
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!();
}

/// One field of a save: `--no-…` clears it, a stated value wins, and an unmentioned one keeps what
/// the agent already had.
///
/// The whole rule, in one place, so no field can quietly disagree with it. Five of them did — every
/// one written out longhand at its own call site, and the mistake was invisible because there was
/// nothing to compare against. A save is a *patch*, not a whole new agent: the caller states what
/// they came to change, and cannot be expected to restate settings they may not know exist.
fn kept<T: Default>(clear: bool, stated: Option<T>, stored: Option<T>) -> T {
    if clear {
        return T::default();
    }
    stated.or(stored).unwrap_or_default()
}

/// The same rule for a boolean, which has no empty value to mean "unmentioned" — so the pair of
/// flags carries it: `--x` on, `--no-x` off, neither leaves it alone.
fn flag(on: bool, off: bool, stored: bool) -> bool {
    on || (!off && stored)
}

/// A repeatable flag's value, or `None` when it was never passed.
///
/// The empty `Vec` clap hands back for an absent flag is indistinguishable from one the caller
/// emptied on purpose, so it is turned into `None` *before* [`kept`] sees it — and `--no-…` is the
/// only way to mean "empty".
fn stated<T>(values: Vec<String>, parse: impl FnOnce(Vec<String>) -> T) -> Option<T> {
    (!values.is_empty()).then(|| parse(values))
}

/// Parse `--secret` values into attachments. Each value is a comma-separated list of
/// `NAME` (global) or `PROJECT/NAME` (project-scoped) references; blanks are dropped.
/// Parse `--env KEY=VALUE` flags into the manifest's env table. The value is everything after the
/// first `=`, so it may contain one itself. `PATH` is rejected rather than accepted-and-ignored:
/// it is assembled at launch from `--path`, so honouring it here would be a lie.
fn parse_env_vars(values: Vec<String>) -> Result<BTreeMap<String, String>, String> {
    let mut vars = BTreeMap::new();
    for value in values {
        let Some((key, value)) = value.split_once('=') else {
            return Err(format!("--env expects KEY=VALUE, got: {value}"));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err("--env needs a variable name before the `=`".to_string());
        }
        if key == "PATH" {
            return Err("--env cannot set PATH; use --path to add directories to it".to_string());
        }
        vars.insert(key.to_string(), value.trim().to_string());
    }
    Ok(vars)
}

fn parse_secret_attachments(values: Vec<String>) -> Vec<SecretAttachment> {
    values
        .iter()
        .flat_map(|value| value.split(','))
        .filter_map(|token| {
            let token = token.trim();
            if token.is_empty() {
                return None;
            }
            let attachment = match token.split_once('/') {
                Some((project, name)) => SecretAttachment {
                    project: Some(project.trim().to_string()),
                    name: name.trim().to_string(),
                },
                None => SecretAttachment {
                    project: None,
                    name: token.to_string(),
                },
            };
            (!attachment.name.is_empty()).then_some(attachment)
        })
        .collect()
}

/// One run in a cross-agent listing: who ran it, when, and what became of it.
#[derive(Debug, serde::Serialize)]
struct RunRow {
    agent: String,
    run_id: String,
    started_at: u64,
    /// `running`, `failed`, `done`, or `unknown` — the one word every engine's verdict is
    /// flattened to, so a filter works without knowing any of their vocabularies.
    status: &'static str,
    /// The engine's own word, kept beside `status` because the distinctions are what a reader acts
    /// on: `api_error` and `aborted_tools` are both failures wanting opposite responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_micro_usd: Option<u64>,
    message: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    result_head: String,
}

impl RunRow {
    fn of(agent: &str, run: RunInfo) -> Self {
        let outcome = run.outcome;
        Self {
            agent: agent.to_string(),
            run_id: run.run_id,
            started_at: run.started_at,
            status: match (run.running, &outcome) {
                (true, _) => "running",
                (false, Some(o)) if o.is_error => "failed",
                (false, Some(o)) if o.is_reported() => "done",
                // Stopped, and nothing is actually known about how: a run from before the store
                // kept outcomes, or one whose log held no telemetry to parse. Not `done` — that
                // would be this listing claiming a run worked on the strength of having noticed
                // it stop.
                (false, _) => "unknown",
            },
            terminal_reason: outcome.as_ref().and_then(|o| o.terminal_reason.clone()),
            duration_ms: outcome.as_ref().and_then(|o| o.duration_ms),
            cost_micro_usd: outcome.as_ref().and_then(|o| o.cost_micro_usd),
            message: title(&run.message),
            result_head: outcome.map(|o| o.result_head).unwrap_or_default(),
        }
    }

    fn print(&self) {
        let reason = self
            .terminal_reason
            .as_deref()
            .filter(|r| *r != self.status)
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        println!("{} {}{}", self.agent, self.status, reason);
        println!("  run: {}", self.run_id);
        let mut facts = Vec::new();
        if let Some(ms) = self.duration_ms {
            facts.push(format!("{:.1}s", ms as f64 / 1000.0));
        }
        if let Some(micro) = self.cost_micro_usd {
            facts.push(format!("${:.2}", micro as f64 / 1_000_000.0));
        }
        if !facts.is_empty() {
            println!("  {}", facts.join("  "));
        }
        println!("  task: {}", self.message);
        if !self.result_head.is_empty() {
            println!("  said: {}", self.result_head);
        }
    }
}

/// `text`'s first line, cut to `width` characters — a listing shows one line per run.
fn title(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    let mut out: String = line.chars().take(100).collect();
    if out.chars().count() < line.chars().count() {
        out.push('…');
    }
    out
}

/// A `90s` / `30m` / `2h` / `7d` window, in milliseconds.
fn parse_since(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let (digits, unit) = text.split_at(text.len().saturating_sub(1));
    let per = match unit {
        "s" => 1_000,
        "m" => 60 * 1_000,
        "h" => 60 * 60 * 1_000,
        "d" => 24 * 60 * 60 * 1_000,
        _ => return Err(format!("--since wants a unit of s/m/h/d, got {text:?}")),
    };
    digits
        .parse::<u64>()
        .map(|n| n * per)
        .map_err(|_| format!("--since wants a number then s/m/h/d, got {text:?}"))
}

/// The moment, in unix milliseconds.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Print an agent definition in the compact human CLI format.
fn print_agent(agent: &StoredAgent) {
    let arguments = agent
        .manifest
        .typed_arguments::<AgentSummaryArguments>()
        .unwrap_or_default();
    println!(
        "{} — {} [{}]",
        agent.name,
        agent.manifest.backend,
        agent.manifest.executor()
    );
    if let Some(model) = arguments.model {
        println!("  model: {model}");
    }
    if let Some(project) = &agent.manifest.project {
        println!("  project: {project}");
    }
    if let Some(tools) = arguments.tools.filter(|tools| !tools.trim().is_empty()) {
        println!("  commands: {tools}");
    }
    if !agent.manifest.bin_tools.is_empty() {
        println!("  tools (.bin): {}", agent.manifest.bin_tools.join(", "));
    }
    if !agent.manifest.secrets.is_empty() {
        let refs: Vec<String> = agent
            .manifest
            .secrets
            .iter()
            .map(|s| match &s.project {
                Some(project) => format!("{project}/{}", s.name),
                None => s.name.clone(),
            })
            .collect();
        println!("  secrets: {}", refs.join(", "));
    }
    if !agent.manifest.knowledge.is_empty() {
        println!("  knowledge: {}", agent.manifest.knowledge.join(", "));
    }
    if agent.manifest.memory {
        println!("  memory: agent:{}/memory", agent.name);
    }
    if !agent.manifest.path.is_empty() {
        println!("  path: {}", agent.manifest.path.join(", "));
    }
    if !agent.manifest.env.is_empty() {
        // Names only: an agent's env table is ordinary config, but printing values into a terminal
        // (and its scrollback) is not what `agents show` is for.
        let names: Vec<&str> = agent.manifest.env.keys().map(String::as_str).collect();
        println!("  env: {}", names.join(", "));
    }
    if !agent.manifest.tags.is_empty() {
        println!("  tags: {}", agent.manifest.tags.join(", "));
    }
    if agent.manifest.starred {
        println!("  starred");
    }
}

/// Block until `run_id` finishes, print what the agent answered, and exit with the run's own
/// status — the whole of `--wait`.
///
/// The wait is a poll rather than a subscription because a run's ending is a property of a process
/// this one did not spawn: the launch is detached, so there is no child here to wait on, and the
/// store's liveness answer is the same one every other reader uses.
///
/// The exit status is the *agent's*, not this command's: a turn the engine reported as failed exits
/// non-zero, so `adi-mono agents run … --wait && deploy` means what it looks like. A run that
/// answered nothing is a failure too — there is no result to have composed anything out of.
fn await_run(store: &Agents, name: &str, run_id: &str) -> Result<(), String> {
    /// Long enough that a multi-minute run costs a handful of store reads, short enough that a
    /// quick one is not held up by the polling itself.
    const LOOK_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

    let agent = store
        .get(name)
        .map_err(|e: AgentsError| e.to_string())?
        .ok_or_else(|| format!("no agent named {name}"))?;

    while store.peek_run(&agent, run_id).running {
        std::thread::sleep(LOOK_EVERY);
    }

    // The answer is the last thing the agent said, which is what a caller composing on this wants —
    // not the log, which carries the engine's own event stream around it.
    let answer = store
        .transcript(&agent, run_id)
        .into_iter()
        .rfind(|turn| turn.role == "assistant");

    match answer {
        Some(turn) => {
            let text = turn.text.trim();
            if !text.is_empty() {
                println!("{text}");
            }
            // `is_error` is the engine's own verdict on the turn, so a run that failed says so in
            // the only way a shell reads.
            if turn.metrics.as_ref().is_some_and(|m| m.is_error) || text.is_empty() {
                std::process::exit(1);
            }
            Ok(())
        }
        None => Err(format!("run {run_id} finished without answering")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule, in the three shapes it comes in. A save is a patch: what the caller did not
    /// mention must survive it.
    #[test]
    fn an_unmentioned_field_survives_the_save() {
        let stored = || Some(vec!["bugbounty".to_string(), "v2".to_string()]);

        // Unmentioned — clap hands back an empty Vec, which must not read as "empty it".
        assert_eq!(kept(false, stated(vec![], clean_tags), stored()), vec!["bugbounty", "v2"]);
        // Stated wins.
        assert_eq!(
            kept(false, stated(vec!["one".into()], clean_tags), stored()),
            vec!["one"]
        );
        // `--no-…` is the only way to actually clear it.
        assert!(kept::<Vec<String>>(true, stated(vec!["one".into()], clean_tags), stored()).is_empty());
        // Nothing stated and nothing stored is empty, not a panic.
        assert!(kept::<Vec<String>>(false, stated(vec![], clean_tags), None).is_empty());
    }

    /// A project decides which database, secrets, and knowledge an agent's runs reach. Dropping it
    /// on an unrelated save moved 62 agents at once; the inner `Option` is the agent's own
    /// "global", the outer is "was it mentioned".
    #[test]
    fn a_project_is_not_lost_by_a_save_that_never_mentions_it() {
        let filed = || Some(Some("bugbounty".to_string()));
        assert_eq!(kept(false, None, filed()), Some("bugbounty".to_string()));
        assert_eq!(
            kept(false, Some(Some("other".to_string())), filed()),
            Some("other".to_string())
        );
        assert_eq!(kept::<Option<String>>(true, None, filed()), None, "--no-project makes it global");
        // An agent that was already global stays global rather than becoming a phantom project.
        assert_eq!(kept::<Option<String>>(false, None, Some(None)), None);
    }

    /// A bare `bool` has no "unmentioned" value of its own, so the pair of flags carries it. This
    /// is what `--starred`, `--unattended`, and `--memory` all now share.
    #[test]
    fn a_boolean_needs_both_flags_to_mean_anything() {
        assert!(flag(true, false, false), "--x turns it on");
        assert!(!flag(false, true, true), "--no-x turns it off");
        assert!(flag(false, false, true), "neither keeps it on");
        assert!(!flag(false, false, false), "neither keeps it off");
    }
}
