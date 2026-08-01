//! Agent manifests, storage, and execution adapters for ADI.
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-agents-doctest-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&tmp);
//! use adi_agents::{Agents, AgentManifest};
//! use adi_agents::arguments::PtyClaudeArguments;
//!
//! # let store = Agents::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let store = Agents::open();
//! let spec = AgentManifest {
//!     backend: "pty:claude".into(),
//!     arguments: PtyClaudeArguments {
//!         model: Some("opus".into()),
//!         ..Default::default()
//!     },
//!     ..Default::default()
//! };
//! let saved = store.save("athz-solver", spec)?;
//! assert_eq!(saved.name, "athz-solver");
//! assert_eq!(saved.manifest.executor(), "pty");
//! assert!(saved.manifest.created_at > 0);
//!
//! assert_eq!(store.list()?.len(), 1);
//! assert!(store.delete("athz-solver")?);
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_agents::Error>(())
//! ```

mod agent;
pub mod arguments;
mod backend;
mod backends;
mod error;
mod events;
mod launch;
mod limits;
mod memo;
pub mod progress;
mod run;
mod tool_help;
pub mod wasm;
mod workspace;

use std::path::PathBuf;

use adi_config::{Config, ConfigFile, now_unix};

pub use agent::{
    Agent, AgentManifest, RawAgentArguments, SecretAttachment, StoredAgent, StoredAgentManifest,
    contains_json_null,
};
pub use backend::Backend;
pub use error::{Error, Result};
pub use events::{
    AgentDeleted, AgentRunDeleted, AgentRunStarted, AgentRunStopped, AgentSaved, event_catalog,
    event_types,
};
pub use limits::{DEFAULT_MAX_CONCURRENT_RUNS, RunLimits, RunLoad};
pub use progress::{
    BackendCapabilities, Step, ToolStatus, TurnContent, TurnMetrics, capabilities,
};
pub use run::{
    Launch, Peek, RunInfo, Sent, Turn, capture_pane, is_runnable, running_sessions, send_keys,
    session_name,
};
pub use wasm::DispatchOutcome;

use agent::validate_name;
use run::{
    adi_turn_in, advance_in, can_advance_in, conversation_dir_in, delete_run_in, is_running_in,
    launch_in, peek_in, peek_run_in, reply_in, runs_in, set_run_hidden_in, stop_in, stop_run_in,
    transcript_in, unqueue_in,
};

const AGENTS_MODULE: &str = "agents";
const WORKFORCE_MODULE: &str = "workforce";
const SESSIONS_MODULE: &str = "sessions";
const MANIFEST_EXT: &str = "toml";

/// An on-disk agent registry.
#[derive(Debug, Clone)]
pub struct Agents {
    config: Config,
}

/// The resolved inputs a launch or reply needs from the store: the sessions/runs dir, the
/// working directory, the `PATH` the run resolves commands on, and the environment it is launched
/// with (its attached secrets, the pointer to its scope's database, where it starts, and its own
/// declared vars).
struct LaunchContext {
    /// The agent as it is *launched* — the stored definition with its tools' own help and its
    /// location folded into the system prompt (see [`tool_help`], [`workspace`]). Launch from this,
    /// never from the stored agent, or the run's `.bin` holds commands its prompt never mentions
    /// and it starts in a directory nothing told it about.
    agent: StoredAgent,
    sessions_dir: PathBuf,
    /// Where this run's process starts — already resolved through [`workspace::resolve`], so it is
    /// the whole answer. Executors spawn into it directly and no longer consult the manifest
    /// themselves; two places deciding one directory is how they drift.
    base_dir: PathBuf,
    /// Already assembled (see [`launch::run_path`]): the agent's `.bin`, the dirs its manifest
    /// declares, and the standard tool dirs. Executors apply it verbatim — after the vars below, so
    /// nothing injected there can shadow it.
    run_path: String,
    run_env: Vec<(String, String)>,
}

impl Default for Agents {
    fn default() -> Self {
        Self::open()
    }
}

impl Agents {
    #[must_use]
    pub fn open() -> Self {
        Self {
            config: Config::open(),
        }
    }

    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.config.module(AGENTS_MODULE).dir().to_path_buf()
    }

    fn agent_file(&self, name: &str) -> ConfigFile<StoredAgentManifest> {
        self.config.module(AGENTS_MODULE).manifest_file(name)
    }

    /// Returns registered agents sorted by name.
    ///
    /// # Errors
    /// Returns store I/O or manifest decoding errors.
    pub fn list(&self) -> Result<Vec<StoredAgent>> {
        let entries = match std::fs::read_dir(self.dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e)),
        };

        let mut agents = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let Ok(file_name) = entry.file_name().into_string() else {
                continue;
            };
            let Some(name) = file_name.strip_suffix(&format!(".{MANIFEST_EXT}")) else {
                continue;
            };
            if validate_name(name).is_err() {
                continue;
            }
            agents.push(Agent {
                name: name.to_string(),
                manifest: self.agent_file(name).load()?,
            });
        }
        agents.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(agents)
    }

    /// # Errors
    /// Returns name validation or manifest decoding errors.
    pub fn get(&self, name: &str) -> Result<Option<StoredAgent>> {
        validate_name(name)?;
        let file = self.agent_file(name);
        if !file.exists() {
            return Ok(None);
        }
        Ok(Some(Agent {
            name: name.to_string(),
            manifest: file.load()?,
        }))
    }

    /// # Errors
    /// Returns errors from [`Self::get`] or argument decoding.
    pub fn get_typed<Args: serde::de::DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<Option<Agent<Args>>> {
        self.get(name)?.map(StoredAgent::into_typed).transpose()
    }

    /// Upserts an agent, preserving `created_at` and stamping `updated_at`.
    ///
    /// # Errors
    /// Returns name, argument, or store errors.
    pub fn save<Args: serde::Serialize>(
        &self,
        name: &str,
        mut manifest: AgentManifest<Args>,
    ) -> Result<Agent<Args>> {
        validate_name(name)?;
        let file = self.agent_file(name);
        let now = now_unix();
        manifest.created_at = file.carried_created_at(now);
        manifest.updated_at = now;
        let stored = manifest.to_stored()?;
        arguments::validate_builtin(&stored)?;
        file.save(&stored)?;
        self.emit(
            "adi.agents.saved",
            &AgentSaved {
                agent: name.to_string(),
            },
        );
        Ok(Agent {
            name: name.to_string(),
            manifest,
        })
    }

    /// Publish an `adi.agents.*` event onto the shared bus. Best-effort and fire-and-forget: this
    /// registry neither knows nor cares whether anything subscribes, and a spool failure must
    /// never fail the lifecycle action that caused it. Emitted against **this store's** [`Config`],
    /// so a scratch store stays isolated.
    fn emit(&self, event: &str, payload: &impl serde::Serialize) {
        if let Ok(json) = serde_json::to_string(payload) {
            let _ = adi_events::Events::with_config(self.config.clone()).emit(event, json);
        }
    }

    /// Renames an agent's manifest, keeping its contents and `created_at` intact.
    ///
    /// The rename is a plain file move, so a following [`Self::save`] under the new name behaves
    /// like any other edit. Renaming a *running* agent is refused: sessions are keyed by name
    /// (`adi-agent-<name>`, `sessions/<executor>/<name>.pid`), so the live session would be
    /// orphaned beyond the reach of stop.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for either name, [`Error::NotFound`] when `from` isn't registered,
    /// [`Error::Exists`] when `to` is taken, [`Error::AlreadyRunning`] when `from` is live.
    pub fn rename(&self, from: &str, to: &str) -> Result<()> {
        validate_name(from)?;
        validate_name(to)?;
        if from == to {
            return Ok(());
        }
        let agent = self
            .get(from)?
            .ok_or_else(|| Error::NotFound(from.to_string()))?;
        if self.agent_file(to).path().exists() {
            return Err(Error::Exists(to.to_string()));
        }
        if self.is_running(&agent) {
            return Err(Error::AlreadyRunning(from.to_string()));
        }
        std::fs::rename(self.agent_file(from).path(), self.agent_file(to).path()).map_err(Error::Io)
    }

    /// How many runs may be live at once — overall and per project — read from
    /// `sessions/settings.toml`. See [`RunLimits`].
    #[must_use]
    pub fn limits(&self) -> RunLimits {
        RunLimits::load(&self.config.module(SESSIONS_MODULE))
    }

    /// Set how many runs may be live at once, returning what was stored.
    ///
    /// # Errors
    /// [`Error::Config`] if the settings file can't be written.
    pub fn set_limits(&self, limits: RunLimits) -> Result<RunLimits> {
        limits.save(&self.config.module(SESSIONS_MODULE))?;
        Ok(limits)
    }

    /// Set (or, with `0`, clear) one project's own cap, leaving the global one alone. Returns the
    /// whole updated set, so a caller sees what it now has.
    ///
    /// # Errors
    /// [`Error::Config`] if the settings file can't be written.
    pub fn set_project_limit(&self, project: &str, max_concurrent_runs: u32) -> Result<RunLimits> {
        let mut limits = self.limits();
        limits.set_project(project, max_concurrent_runs);
        self.set_limits(limits)
    }

    /// What is running right now, totalled and divided between projects — one walk of the run
    /// directories, weighed against [`Self::limits`].
    ///
    /// The agent list is only read when something is actually running, so the common case (an idle
    /// machine asking whether it may start something) costs a directory scan and nothing else. A pty
    /// session keeps no run directory to name an agent from, so live sessions are matched against
    /// the agent list instead — again, only when a session is up.
    #[must_use]
    pub fn run_load(&self) -> RunLoad {
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        let mut by_agent = run::running_by_agent_in(&sessions_dir);
        let sessions = run::running_sessions();
        if by_agent.is_empty() && sessions.is_empty() {
            return RunLoad::default();
        }
        let agents = self.list().unwrap_or_default();
        for agent in &agents {
            if sessions.contains(&run::session_name(&agent.name)) {
                *by_agent.entry(agent.name.clone()).or_default() += 1;
            }
        }
        RunLoad::new(
            &by_agent,
            agents
                .into_iter()
                .map(|a| (a.name, a.manifest.project.clone())),
        )
    }

    /// How many runs are live right now, across every agent and backend.
    #[must_use]
    pub fn running_count(&self) -> usize {
        self.run_load().total()
    }

    /// Whether a run of `agent` would be refused right now — the global cap is full, or its
    /// project's is. Best-effort by construction: runs are launched from several processes (the app,
    /// the CLI, a trigger's `adi-agents run`), so two launches racing at the boundary can both see a
    /// free slot. The caps throttle the steady state; they are not semaphores.
    #[must_use]
    pub fn at_capacity_for(&self, agent: &StoredAgent) -> bool {
        self.full_cap_for(agent, &self.limits(), &self.run_load())
            .is_some()
    }

    /// Which cap `agent` is up against, if either — the global one (`None` project) or its own
    /// project's. The single place both gates are decided, so a launch, a reply, and a page render
    /// can never disagree about what is full.
    fn full_cap_for(
        &self,
        agent: &StoredAgent,
        limits: &RunLimits,
        load: &RunLoad,
    ) -> Option<Error> {
        if limits.is_full(load.total()) {
            return Some(Error::TooManyRunning {
                project: None,
                running: load.total(),
                limit: limits.max_concurrent_runs,
            });
        }
        let project = agent.manifest.project.as_deref()?;
        let limit = limits.project_limit(project)?;
        let running = load.in_project(project);
        (running >= limit as usize).then(|| Error::TooManyRunning {
            project: Some(project.to_string()),
            running,
            limit,
        })
    }

    /// # Errors
    /// Returns [`Error::NotFound`], [`Error::TooManyRunning`], or backend launch errors.
    pub fn run(&self, name: &str) -> Result<Launch> {
        self.run_with_message(name, "run")
    }

    /// # Errors
    /// Returns [`Error::NotFound`], [`Error::TooManyRunning`], or backend launch errors.
    pub fn run_with_message(&self, name: &str, message: &str) -> Result<Launch> {
        self.run_in(name, message, None)
    }

    /// Launch a run in a directory chosen for *this* launch, rather than the one its manifest
    /// implies. The answer for an agent whose definition is reused across many targets — a recon
    /// pass, a per-repo reviewer — where the right directory is a property of the run and no stored
    /// field can hold it. `None` behaves exactly like [`Self::run_with_message`]; a harness
    /// conversation pins whatever is resolved here, so its replies re-enter the same directory.
    ///
    /// Subject to the [run cap](RunLimits): with as many runs already live as the store allows, this
    /// refuses rather than adding one more. [`Self::force_run_in`] is the same launch with the cap
    /// waived — what a human who has read the refusal and asked again gets.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`], [`Error::TooManyRunning`], or backend launch errors.
    pub fn run_in(&self, name: &str, message: &str, working_dir: Option<&str>) -> Result<Launch> {
        self.launch_run(name, message, working_dir, false)
    }

    /// Launch a run whatever else is running — the deliberate override of the [run cap](RunLimits),
    /// for a human who wants this one now. Never taken automatically: a trigger, a queued turn, or
    /// anything else the platform starts on its own goes through [`Self::run_in`] and waits its turn.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`] or backend launch errors.
    pub fn force_run_in(
        &self,
        name: &str,
        message: &str,
        working_dir: Option<&str>,
    ) -> Result<Launch> {
        self.launch_run(name, message, working_dir, true)
    }

    /// The one launch path: resolve the agent, weigh the cap unless `force`, spawn, announce.
    fn launch_run(
        &self,
        name: &str,
        message: &str,
        working_dir: Option<&str>,
        force: bool,
    ) -> Result<Launch> {
        let agent = self
            .get(name)?
            .ok_or_else(|| Error::NotFound(name.to_string()))?;
        if !force
            && let Some(full) = self.full_cap_for(&agent, &self.limits(), &self.run_load())
        {
            return Err(full);
        }
        let ctx = self.launch_context(&agent, working_dir);
        let launch = launch_in(
            &ctx.agent,
            &ctx.sessions_dir,
            &ctx.base_dir,
            &ctx.run_path,
            message,
            &ctx.run_env,
        )?;
        self.emit(
            "adi.agents.run.started",
            &AgentRunStarted::of(name, message, &launch),
        );
        Ok(launch)
    }

    /// Say something into one of a harness agent's conversations (`run_id` is the conversation id).
    /// One turn runs at a time, so this either starts the next turn or queues the message behind the
    /// answer still in flight — see [`Sent`].
    ///
    /// A full [run cap](RunLimits) queues rather than refuses: the message keeps its place and
    /// starts when a slot frees, which is what a conversation's queue already does for the answer in
    /// flight. Nothing typed into a chat is ever lost to the limit.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`] or backend launch errors — including [`Error::NotRunnable`] for a
    /// backend that keeps no conversation.
    pub fn reply(&self, name: &str, conv_id: &str, message: &str) -> Result<Sent> {
        let agent = self
            .get(name)?
            .ok_or_else(|| Error::NotFound(name.to_string()))?;
        let may_start = !self.at_capacity_for(&agent);
        let dir = self.conversation_dir(&agent, conv_id);
        let ctx = self.launch_context(&agent, dir.to_str());
        let sent = reply_in(
            &ctx.agent,
            &ctx.sessions_dir,
            &ctx.base_dir,
            &ctx.run_path,
            conv_id,
            message,
            &ctx.run_env,
            may_start,
        )?;
        if let Sent::Started(launch) = &sent {
            self.emit(
                "adi.agents.run.started",
                &AgentRunStarted::of(name, message, launch),
            );
        }
        Ok(sent)
    }

    /// A harness conversation's transcript, oldest first (empty for backends that keep no
    /// conversation, or for an unknown conversation id). Trailing `queued` turns are the messages
    /// still waiting their place in the queue.
    ///
    /// Reading a conversation also *advances* it: if the last answer has landed and something is
    /// queued behind it, that message's turn starts here. Nothing else moves the queue — the same
    /// lazy clock that settles a finished answer.
    #[must_use]
    pub fn transcript(&self, agent: &StoredAgent, conv_id: &str) -> Vec<Turn> {
        self.advance_queue(agent, conv_id);
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        transcript_in(agent, &sessions_dir, conv_id)
    }

    /// Drop one message from a conversation's queue by its position, for something you have thought
    /// better of before it was ever asked. Returns whether there was one there to drop.
    ///
    /// # Errors
    /// Returns name validation errors.
    pub fn unqueue(&self, name: &str, conv_id: &str, index: usize) -> Result<bool> {
        validate_name(name)?;
        let Some(agent) = self.get(name)? else {
            return Ok(false);
        };
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        Ok(unqueue_in(&agent, &sessions_dir, conv_id, index))
    }

    /// Start a conversation's next queued message, if it is idle and one is waiting; reports whether
    /// a turn started. The cheap gate comes first so an idle poll — which is nearly every poll —
    /// costs one `exists` rather than the tool sync a launch context pays for.
    ///
    /// This is the platform starting a run by itself, so the [run cap](RunLimits) binds absolutely:
    /// at capacity the message stays at the head of its queue and the next poll asks again.
    fn advance_queue(&self, agent: &StoredAgent, conv_id: &str) -> bool {
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        if !can_advance_in(agent, &sessions_dir, conv_id) {
            return false;
        }
        if self.at_capacity_for(agent) {
            return false;
        }
        let dir = self.conversation_dir(agent, conv_id);
        let ctx = self.launch_context(agent, dir.to_str());
        let Some((message, launch)) = advance_in(
            &ctx.agent,
            &ctx.sessions_dir,
            &ctx.base_dir,
            &ctx.run_path,
            conv_id,
            &ctx.run_env,
        ) else {
            return false;
        };
        self.emit(
            "adi.agents.run.started",
            &AgentRunStarted::of(&agent.name, &message, &launch),
        );
        true
    }

    /// Run one turn of a `harness:adi` conversation: read its transcript, call the configured model
    /// provider, and return the answer text. Invoked by the `adi-mono harness-turn` child that an
    /// `adi` turn spawns; that child prints the returned text, which the conversation folds into the
    /// transcript as the assistant's answer.
    ///
    /// This blocks on the provider HTTP call, so it must run in a sync context (the CLI child), never
    /// inside the app server's async runtime.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`] for an unknown agent, or argument / provider-configuration /
    /// HTTP / decoding errors from the loop.
    pub fn run_adi_turn(
        &self,
        agent_name: &str,
        conv_id: &str,
        sink: crate::backends::adi_events::Sink<'_>,
    ) -> Result<String> {
        let agent = self
            .get(agent_name)?
            .ok_or_else(|| Error::NotFound(agent_name.to_string()))?;
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        // The `adi` loop builds its own system message here, in the turn child — not from the argv
        // the launch assembled — so this is where its tool help and its location have to be folded
        // in. The child was spawned *into* the run's directory and carries it in its environment,
        // so `current` reproduces it exactly, per-run choice included.
        let workdir = workspace::current(&self.config, &agent.manifest);
        adi_turn_in(&self.decorated(&agent, &workdir), &sessions_dir, conv_id, sink)
    }

    /// Where a turn of `conv_id` must run: the directory that conversation started in. Every turn
    /// re-enters it, so a thread stays in one place for its whole life — the Claude session store is
    /// keyed by cwd, so answering from elsewhere would leave the session unresumable, and the files
    /// earlier turns wrote by relative path are only where they are.
    ///
    /// A conversation from before the directory was recorded gets the **store root**, which is
    /// where every run started back then — not a fresh resolve, which would now hand a
    /// project-scoped agent its project directory and strand its existing threads.
    fn conversation_dir(&self, agent: &StoredAgent, conv_id: &str) -> PathBuf {
        conversation_dir_in(agent, self.config.module(SESSIONS_MODULE).dir(), conv_id)
            .unwrap_or_else(|| self.config.root().to_path_buf())
    }

    /// The shared per-launch context: where runs live, the cwd, the agent's `.bin`, and the
    /// environment its run gets — resolved the same way whether launching a fresh run or answering
    /// a reply.
    ///
    /// `run_dir` is this launch's own working directory, for a caller pointing one agent at a
    /// different target each run; `None` leaves the manifest and the agent's project to decide. See
    /// [`workspace::resolve`] for the full precedence. A conversation's replies must run in the same
    /// cwd as its first turn, so the engine's session store lines up — the conversation pins the
    /// directory it started in and re-enters that, whatever is resolved here later.
    fn launch_context(&self, agent: &StoredAgent, run_dir: Option<&str>) -> LaunchContext {
        let base_dir = workspace::resolve(&self.config, &agent.manifest, run_dir);
        // Best-effort: a sync failure (or no tools) just means no extra bin on PATH, never a blocked run.
        let bin_dir = adi_tools::Tools::with_config(self.config.clone())
            .sync_agent_bin(&agent.name, &agent.manifest.bin_tools)
            .ok();
        // Allowlist only — nothing pulled in from a scope just for existing. Resolved against this
        // store's Config (test stores stay isolated). Best-effort: a missing/undecryptable secret is skipped.
        let mut run_env = attached_secret_env(&self.config, &agent.manifest.secrets);
        // Point the run at its scope's shared database (a project agent gets that project's), so
        // `adi-db` in its shell and `import … from "@adi/db"` in its `ts` code both resolve without
        // the agent being told where the file is. Secrets are listed first, so a secret named
        // `ADI_DB` would be overridden here rather than silently redirecting the run's database.
        run_env.extend(self.config.db_env(agent.manifest.project.as_deref()));
        // Where the run starts, what it is called, and what it is scoped to. A run that has to be
        // *told* its directory in prose gets it wrong; a script it writes can't be told at all.
        run_env.extend(workspace::env(&self.config, agent, &base_dir));
        // The agent's own declared vars last, so an explicit `[env]` entry wins over a secret or
        // the database pointer it collides with — it is the most specific statement about this
        // agent there is. `PATH` is dropped rather than honoured: it is built from the manifest's
        // `path` below and applied after every var, so a `PATH` here would be silently overridden.
        run_env.extend(
            agent
                .manifest
                .env
                .iter()
                .filter(|(key, _)| key.as_str() != "PATH")
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        LaunchContext {
            agent: self.decorated(agent, &base_dir),
            sessions_dir: self.config.module(SESSIONS_MODULE).dir().to_path_buf(),
            base_dir,
            run_path: launch::run_path(bin_dir.as_deref(), &agent.manifest.path),
            run_env,
        }
    }

    /// The agent as it should be launched: its stored definition with two things it otherwise has no
    /// way to know appended to its system prompt — the enabled tools' own help, so it knows what the
    /// commands on its `PATH` are and how to call them, and where the run starts, so it neither
    /// guesses at a directory nor `cd`s into one. The user's prompt is untouched and leads; nothing
    /// is written back to the store.
    ///
    /// Best-effort at every step — this decorates a prompt and must never cost a run. A backend
    /// whose `system_prompt` isn't one (see [`tool_help::applies_to`]), an agent with no tools, or a
    /// decorated manifest its own backend would reject: each falls back to launching the agent
    /// exactly as stored.
    fn decorated(&self, agent: &StoredAgent, workdir: &std::path::Path) -> StoredAgent {
        if !tool_help::applies_to(&agent.manifest.backend) {
            return agent.clone();
        }
        let mut decorated = agent.clone();
        // Location first: it is the shorter block and the one that governs every path in the
        // sections above and below it.
        tool_help::fold_into(
            &mut decorated.manifest.arguments,
            &workspace::block(&self.config, agent, workdir),
        );
        if !agent.manifest.bin_tools.is_empty() {
            let tools = adi_tools::Tools::with_config(self.config.clone());
            if let Some(block) = tool_help::block(&tools.help_for(&agent.manifest.bin_tools)) {
                tool_help::fold_into(&mut decorated.manifest.arguments, &block);
            }
        }
        // A backend whose arguments are `deny_unknown_fields` would reject a `system_prompt` it has
        // no field for. None do today; if one ever doesn't, its runs keep working undecorated
        // rather than failing at launch.
        if arguments::validate_builtin(&decorated.manifest).is_err() {
            return agent.clone();
        }
        decorated
    }

    /// Dispatches a message synchronously to a `wasm:*` agent.
    ///
    /// # Errors
    /// Returns lookup, backend, component loading, or dispatch errors.
    pub fn run_wasm(
        &self,
        name: &str,
        handler: Option<&str>,
        message: &str,
    ) -> Result<DispatchOutcome> {
        let agent = self
            .get(name)?
            .ok_or_else(|| Error::NotFound(name.to_string()))?;
        if !wasm::is_wasm(&agent) {
            return Err(Error::NotRunnable(agent.manifest.backend.to_string()));
        }
        let workforce_dir = self.config.module(WORKFORCE_MODULE).dir().to_path_buf();
        wasm::dispatch(&agent, &workforce_dir, handler, message)
    }

    #[must_use]
    pub fn is_running(&self, agent: &StoredAgent) -> bool {
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        is_running_in(agent, &sessions_dir)
    }

    /// A read-only live snapshot of an agent for the live view: a pty screen capture for interactive
    /// backends, or the latest run's log tail for the headless backends.
    #[must_use]
    pub fn peek(&self, agent: &StoredAgent) -> Peek {
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        peek_in(agent, &sessions_dir)
    }

    /// The run history of a headless agent, newest first (empty for interactive backends, whose
    /// live session is their only "run").
    ///
    /// Like [`Self::transcript`], listing advances any conversation whose answer has landed with
    /// something still queued behind it — so a queue keeps moving while you are reading some *other*
    /// chat, not only the one you have open.
    #[must_use]
    pub fn runs(&self, agent: &StoredAgent) -> Vec<RunInfo> {
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        let runs = runs_in(agent, &sessions_dir);
        let idle: Vec<String> = runs
            .iter()
            .filter(|r| !r.running)
            .map(|r| r.run_id.clone())
            .collect();
        let advanced = idle.iter().fold(false, |any, conv_id| {
            self.advance_queue(agent, conv_id) || any
        });
        // Only re-read when something actually started — the answer must not report a conversation
        // as idle in the very breath it started its next turn.
        if advanced {
            return runs_in(agent, &sessions_dir);
        }
        runs
    }

    /// A read-only snapshot of one specific run of a headless agent (or the pty screen, for an
    /// interactive backend, where `run_id` is ignored).
    #[must_use]
    pub fn peek_run(&self, agent: &StoredAgent, run_id: &str) -> Peek {
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        peek_run_in(agent, &sessions_dir, run_id)
    }

    /// Stops one specific run of an agent, returning whether a live run was found.
    ///
    /// # Errors
    /// Returns name validation or backend lifecycle errors.
    pub fn stop_run(&self, name: &str, run_id: &str) -> Result<bool> {
        validate_name(name)?;
        let Some(agent) = self.get(name)? else {
            return Ok(false);
        };
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        let stopped = stop_run_in(&agent, &sessions_dir, run_id)?;
        if stopped {
            self.emit(
                "adi.agents.run.stopped",
                &AgentRunStopped {
                    agent: name.to_string(),
                    run_id: Some(run_id.to_string()),
                },
            );
        }
        Ok(stopped)
    }

    /// Delete one run of an agent outright — for a harness backend, a whole conversation: its
    /// transcript, its log, its queue, all of it. A live run is stopped first, so nothing is left
    /// writing into a slot that no longer exists. Returns whether there was a run there to delete;
    /// deleting one that is already gone is not an error, so a repeated click settles quietly.
    ///
    /// This is not [`Self::stop_run`]: stopping ends a run but leaves it in the history to read,
    /// while this removes it from the history entirely.
    ///
    /// # Errors
    /// Returns name validation errors, [`Error::Unsupported`] for a backend that keeps no run
    /// history, or lifecycle errors from stopping a live run.
    pub fn delete_run(&self, name: &str, run_id: &str) -> Result<bool> {
        validate_name(name)?;
        let Some(agent) = self.get(name)? else {
            return Ok(false);
        };
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        let deleted = delete_run_in(&agent, &sessions_dir, run_id)?;
        if deleted {
            self.emit(
                "adi.agents.run.deleted",
                &AgentRunDeleted {
                    agent: name.to_string(),
                    run_id: run_id.to_string(),
                },
            );
        }
        Ok(deleted)
    }

    /// Hide one run from the chat rail, or bring it back (`hidden: false`). Returns whether there was
    /// a run there to flag; flagging one that is already gone is not an error.
    ///
    /// This is not [`Self::delete_run`]: nothing is removed and nothing is stopped. The flag rides in
    /// the run's metadata, so a hidden session stays out of the rail across reloads — and is still
    /// listed by [`Self::runs`], for the views that want the whole history.
    ///
    /// # Errors
    /// Returns name validation errors, or [`Error::Unsupported`] for a backend that keeps no run
    /// history (a pty session is not a run, so there is nothing to mark).
    pub fn set_run_hidden(&self, name: &str, run_id: &str, hidden: bool) -> Result<bool> {
        validate_name(name)?;
        let Some(agent) = self.get(name)? else {
            return Ok(false);
        };
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        set_run_hidden_in(&agent, &sessions_dir, run_id, hidden)
    }

    /// Stops a run, returning whether one was found.
    ///
    /// # Errors
    /// Returns name validation or backend lifecycle errors.
    pub fn stop(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        let Some(agent) = self.get(name)? else {
            return Ok(false);
        };
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        let stopped = stop_in(&agent, &sessions_dir)?;
        if stopped {
            self.emit(
                "adi.agents.run.stopped",
                &AgentRunStopped {
                    agent: name.to_string(),
                    run_id: None,
                },
            );
        }
        Ok(stopped)
    }

    /// # Errors
    /// Returns name validation or store errors.
    pub fn delete(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        let removed = self.config.module(AGENTS_MODULE).remove_manifest(name)?;
        if removed {
            self.emit(
                "adi.agents.deleted",
                &AgentDeleted {
                    agent: name.to_string(),
                },
            );
        }
        Ok(removed)
    }
}

/// Resolve an agent's attached-secret allowlist into `(env-var, value)` pairs for a run. Only the
/// listed secrets are decrypted — nothing is inherited from a scope for merely existing. A global
/// attachment resolves ahead of a project-scoped one, so a project secret overrides a global of
/// the same name (matching [`adi_secrets::Secrets::resolve`]'s precedence). Best-effort: a secret
/// that is missing or fails to decrypt is skipped rather than aborting the run. An empty allowlist
/// short-circuits, so a secrets-free agent never touches the master key.
fn attached_secret_env(config: &Config, attachments: &[SecretAttachment]) -> Vec<(String, String)> {
    if attachments.is_empty() {
        return Vec::new();
    }
    let secrets = adi_secrets::Secrets::with_config(config.clone());
    // Stable sort by scope: globals (`false`) before project-scoped (`true`), so the latter win
    // on a name collision when inserted into the map below.
    let mut ordered: Vec<&SecretAttachment> = attachments.iter().collect();
    ordered.sort_by_key(|a| a.project.is_some());
    let mut env: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for att in ordered {
        if let Ok(Some(value)) = secrets.reveal(att.project.as_deref(), &att.name) {
            env.insert(att.name.clone(), value);
        }
    }
    env.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct CloudManifest {
        region: String,
        replicas: u32,
    }

    #[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
    struct TestArguments {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_mode: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        temperature: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cloud_manifest: Option<CloudManifest>,
    }

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct PartialArguments {
        system_prompt: String,
        max_turns: u64,
        provider: String,
    }

    fn scratch(tag: &str) -> Agents {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Agents::with_config(Config::with_root(root))
    }

    fn spec(backend: &str) -> AgentManifest<TestArguments> {
        AgentManifest {
            backend: backend.into(),
            ..AgentManifest::default()
        }
    }

    #[test]
    fn save_then_get_and_list_round_trip() {
        let store = scratch("crud");
        assert!(store.list().expect("empty list").is_empty());

        let mut m = spec("cloud:worker");
        m.arguments.system_prompt = Some("You are a solver.".into());
        m.arguments.model = Some("opus".into());
        m.arguments.permission_mode = Some("default".into());
        m.tags = vec!["athz".into()];
        m.project = Some("demo".into());
        m.arguments.resume = Some(true);
        m.arguments.cloud_manifest = Some(CloudManifest {
            region: "eu-west-1".into(),
            replicas: 2,
        });
        let saved = store.save("athz-solver", m).expect("save");
        assert_eq!(saved.name, "athz-solver");
        assert_eq!(saved.manifest.arguments.model.as_deref(), Some("opus"));
        assert_eq!(saved.manifest.project.as_deref(), Some("demo"));
        assert_eq!(saved.manifest.arguments.resume, Some(true));
        assert_eq!(
            saved
                .manifest
                .arguments
                .cloud_manifest
                .as_ref()
                .map(|manifest| manifest.replicas),
            Some(2)
        );
        assert!(saved.manifest.created_at > 0);

        let raw =
            std::fs::read_to_string(store.dir().join("athz-solver.toml")).expect("stored manifest");
        let arguments_section = raw.find("[arguments]").expect("arguments table");
        let adi_fields = &raw[..arguments_section];
        assert!(
            !adi_fields
                .lines()
                .any(|line| line.starts_with("system_prompt ="))
        );
        assert!(!adi_fields.lines().any(|line| line.starts_with("model =")));

        let got = store
            .get_typed::<TestArguments>("athz-solver")
            .expect("get")
            .expect("present");
        assert_eq!(got, saved);
        assert_eq!(store.list().expect("list").len(), 1);
    }

    #[test]
    fn save_is_an_upsert_that_preserves_created_at() {
        let store = scratch("upsert");
        let first = store.save("a", spec("process:codex")).expect("create");
        let created = first.manifest.created_at;
        assert!(created > 0);

        let mut edited = spec("harness:adi");
        edited.arguments.temperature = Some(0.2);
        let second = store.save("a", edited).expect("update");
        assert_eq!(second.manifest.backend, Backend::from("harness:adi"));
        assert_eq!(second.manifest.arguments.temperature, Some(0.2));
        assert_eq!(second.manifest.created_at, created);
        assert_eq!(store.list().expect("list").len(), 1);
    }

    #[test]
    fn partial_toml_manifest_uses_struct_defaults() {
        let store = scratch("partial-default");
        std::fs::create_dir_all(store.dir()).expect("agents dir");
        std::fs::write(
            store.dir().join("partial.toml"),
            "starred = true\n\n[arguments]\nsystem_prompt = \"A prompt\"\nmax_turns = 4\nprovider = \"anthropic\"\n",
        )
        .expect("partial manifest");

        let manifest = store
            .get("partial")
            .expect("get")
            .expect("present")
            .manifest;
        assert!(manifest.starred);
        assert_eq!(manifest.backend, Backend::default());
        let typed = manifest
            .clone()
            .into_typed::<PartialArguments>()
            .expect("typed manifest");
        assert_eq!(typed.arguments.system_prompt, "A prompt");
        assert_eq!(typed.arguments.max_turns, 4);
        assert_eq!(typed.arguments.provider, "anthropic");
        assert!(manifest.tags.is_empty());
        assert_eq!(manifest.project, None);
        assert_eq!(manifest.created_at, 0);
        assert_eq!(manifest.updated_at, 0);
    }

    #[test]
    fn delete_removes_the_agent() {
        let store = scratch("delete");
        store.save("gone", spec("pty:claude")).expect("create");
        assert!(store.delete("gone").expect("delete"));
        assert!(store.get("gone").expect("get").is_none());
        assert!(!store.delete("gone").expect("delete missing"));
    }

    #[test]
    fn harness_claude_sdk_saves_via_the_raw_ui_path_and_is_runnable() {
        // Mirror what the web app / CLI submit: a raw argument map, with numeric knobs encoded as
        // floats (the form runs every number through `parse::<f64>()`).
        let store = scratch("harness-raw");
        let mut arguments = RawAgentArguments::new();
        arguments.insert("model".into(), "claude-opus-4-8".into());
        arguments.insert("permission_mode".into(), "plan".into());
        arguments.insert("max_turns".into(), serde_json::json!(20.0));
        arguments.insert("tools".into(), "tasks,projects".into());
        let manifest = AgentManifest {
            backend: "harness:claude-sdk".into(),
            arguments,
            ..StoredAgentManifest::default()
        };

        let saved = store.save("planner", manifest).expect("save harness agent");
        assert_eq!(saved.manifest.backend, Backend::HarnessClaudeSdk);

        let stored = store
            .get("planner")
            .expect("get")
            .expect("present")
            .manifest;
        assert!(is_runnable(&stored), "harness:claude-sdk must be runnable");

        let typed = store
            .get_typed::<crate::arguments::HarnessClaudeSdkArguments>("planner")
            .expect("typed get")
            .expect("present");
        assert_eq!(typed.manifest.arguments.max_turns, Some(20));
        assert_eq!(
            typed.manifest.arguments.tools.as_deref(),
            Some("tasks,projects")
        );
    }

    #[test]
    fn harness_adi_runs_once_it_names_a_provider_and_not_before() {
        let store = scratch("harness-adi-raw");
        let save = |name: &str, arguments: RawAgentArguments| {
            let manifest = AgentManifest {
                backend: "harness:adi".into(),
                arguments,
                ..StoredAgentManifest::default()
            };
            store.save(name, manifest).expect("save adi harness agent");
            store.get(name).expect("get").expect("present").manifest
        };

        // A provider plus its knobs: stored through the raw UI path, typed on the way back, and
        // runnable — every provider the manifest can name is implemented.
        let mut arguments = RawAgentArguments::new();
        arguments.insert("provider".into(), "gemini".into());
        arguments.insert("temperature".into(), serde_json::json!(0.7));
        arguments.insert("max_tokens".into(), serde_json::json!(4096.0));
        let stored = save("adi-agent", arguments);
        assert_eq!(stored.backend, Backend::HarnessAdi);
        assert!(is_runnable(&stored), "a configured adi agent is runnable");

        // No provider is the not-yet-configured case, and stays unrunnable.
        let blank = save("adi-agent-blank", RawAgentArguments::new());
        assert!(
            !is_runnable(&blank),
            "harness:adi with no provider isn't configured yet"
        );
    }

    #[test]
    fn built_in_backends_reject_unknown_arguments_on_save() {
        #[derive(Default, serde::Serialize)]
        struct MisspelledCodexArguments {
            max_truns: u64,
        }

        let store = scratch("strict-built-in");
        let manifest = AgentManifest {
            backend: "process:codex".into(),
            arguments: MisspelledCodexArguments { max_truns: 4 },
            ..AgentManifest::default()
        };
        assert!(matches!(
            store.save("typo", manifest),
            Err(Error::Arguments(message)) if message.contains("max_truns")
        ));
    }

    #[test]
    fn invalid_names_never_touch_disk() {
        let store = scratch("invalid");
        assert!(matches!(store.get("../escape"), Err(Error::InvalidName(_))));
        assert!(matches!(
            store.save("a/b", spec("pty:claude")),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(store.delete(".."), Err(Error::InvalidName(_))));
    }

    #[test]
    fn rename_moves_the_manifest_and_leaves_no_orphan() {
        let store = scratch("rename");
        let mut m = spec("pty:claude");
        m.arguments.model = Some("opus".into());
        m.tags = vec!["athz".into()];
        let created = store.save("old", m).expect("save").manifest.created_at;

        store.rename("old", "new").expect("rename");

        assert!(store.get("old").expect("old gone").is_none());
        let moved = store
            .get_typed::<TestArguments>("new")
            .expect("load renamed")
            .expect("renamed agent exists");
        assert_eq!(moved.manifest.arguments.model.as_deref(), Some("opus"));
        assert_eq!(moved.manifest.tags, vec!["athz".to_string()]);
        // A move, so the agent keeps its original age.
        assert_eq!(moved.manifest.created_at, created);
        assert_eq!(store.list().expect("list").len(), 1);
    }

    #[test]
    fn rename_refuses_to_clobber_an_existing_agent() {
        let store = scratch("rename-clash");
        store.save("one", spec("pty:claude")).expect("save one");
        store.save("two", spec("process:codex")).expect("save two");

        assert!(matches!(
            store.rename("one", "two"),
            Err(Error::Exists(name)) if name == "two"
        ));
        let two = store.get("two").expect("get two").expect("two exists");
        assert_eq!(two.manifest.backend, "process:codex".into());
        assert_eq!(store.list().expect("list").len(), 2);
    }

    #[test]
    fn secrets_attachment_round_trips_and_stores_as_array_of_tables() {
        let store = scratch("secret-attach");
        let mut m = spec("process:claude");
        m.secrets = vec![
            SecretAttachment {
                project: None,
                name: "API_KEY".into(),
            },
            SecretAttachment {
                project: Some("proj".into()),
                name: "DB_URL".into(),
            },
        ];
        store.save("a", m).expect("save with secrets");

        let got = store.get("a").expect("get").expect("present").manifest;
        assert_eq!(got.secrets.len(), 2);
        assert_eq!(got.secrets[0].project, None);
        assert_eq!(got.secrets[0].name, "API_KEY");
        assert_eq!(got.secrets[1].project.as_deref(), Some("proj"));
        assert_eq!(got.secrets[1].name, "DB_URL");

        // The attachment list is stored as a valid TOML array-of-tables (proven to round-trip by
        // the load above, since `toml::from_str` parsed it back into the two attachments).
        let raw = std::fs::read_to_string(store.dir().join("a.toml")).expect("stored manifest");
        assert!(
            raw.contains("[[secrets]]"),
            "expected array-of-tables in {raw}"
        );
        assert!(raw.contains("name = \"API_KEY\""));
        assert!(raw.contains("project = \"proj\""));
    }

    /// The run environment survives the store round-trip, `[env]` included — a TOML table among
    /// scalar fields is the one shape a manifest can get wrong on serialization.
    #[test]
    fn the_run_environment_round_trips_through_the_manifest() {
        let store = scratch("run-env");
        let mut m = spec("process:claude");
        m.path = vec!["$HOME/.nvm/versions/node/v22.14.0/bin".into()];
        m.env = [("NODE_ENV".to_string(), "development".to_string())]
            .into_iter()
            .collect();
        store.save("a", m).expect("save with a run environment");

        let got = store.get("a").expect("get").expect("present").manifest;
        assert_eq!(
            got.path,
            vec!["$HOME/.nvm/versions/node/v22.14.0/bin".to_string()]
        );
        assert_eq!(
            got.env.get("NODE_ENV").map(String::as_str),
            Some("development")
        );

        let raw = std::fs::read_to_string(store.dir().join("a.toml")).expect("stored manifest");
        assert!(raw.contains("[env]"), "expected an env table in {raw}");
        assert!(raw.contains("path = ["), "expected a path array in {raw}");
    }

    /// An agent that declares neither keeps the manifest it had before these fields existed.
    #[test]
    fn an_agent_with_no_run_environment_stores_neither_key() {
        let store = scratch("no-run-env");
        store.save("a", spec("process:claude")).expect("save");
        let raw = std::fs::read_to_string(store.dir().join("a.toml")).expect("stored manifest");
        assert!(!raw.contains("[env]"), "{raw}");
        assert!(!raw.contains("path = "), "{raw}");
    }

    #[test]
    fn an_agent_with_no_attachments_stores_no_secrets_table() {
        let store = scratch("no-secret-attach");
        store.save("a", spec("process:claude")).expect("save");
        let raw = std::fs::read_to_string(store.dir().join("a.toml")).expect("stored manifest");
        // The empty allowlist is skipped on serialization, so pre-secrets manifests are unchanged.
        assert!(!raw.contains("[[secrets]]"));
    }

    #[test]
    fn only_attached_secrets_are_injected_project_scope_winning() {
        let store = scratch("attached-env");
        let secrets = adi_secrets::Secrets::with_config(store.config().clone());
        secrets.set(None, "GLOBAL_ONLY", "g", None).expect("g");
        secrets
            .set(None, "SHARED", "global", None)
            .expect("shared-g");
        secrets
            .set(None, "NOT_ATTACHED", "ambient", None)
            .expect("ambient");
        secrets
            .set(Some("proj"), "PROJ_ONLY", "p", None)
            .expect("p");
        secrets
            .set(Some("proj"), "SHARED", "project", None)
            .expect("shared-p");

        let attachments = vec![
            SecretAttachment {
                project: None,
                name: "GLOBAL_ONLY".into(),
            },
            SecretAttachment {
                project: Some("proj".into()),
                name: "PROJ_ONLY".into(),
            },
            // The same key exists in both scopes; the project one must win.
            SecretAttachment {
                project: None,
                name: "SHARED".into(),
            },
            SecretAttachment {
                project: Some("proj".into()),
                name: "SHARED".into(),
            },
            // A dangling reference is skipped, not fatal.
            SecretAttachment {
                project: None,
                name: "MISSING".into(),
            },
        ];
        let env: std::collections::BTreeMap<String, String> =
            attached_secret_env(store.config(), &attachments)
                .into_iter()
                .collect();

        assert_eq!(env.get("GLOBAL_ONLY").map(String::as_str), Some("g"));
        assert_eq!(env.get("PROJ_ONLY").map(String::as_str), Some("p"));
        assert_eq!(env.get("SHARED").map(String::as_str), Some("project"));
        assert!(!env.contains_key("MISSING"));
        // The allowlist is exclusive: a secret that exists but isn't attached is never injected.
        assert!(!env.contains_key("NOT_ATTACHED"));
    }

    /// End to end through the launch context: what an agent declares is what its run is started
    /// with — the dirs on `PATH`, the vars in the environment.
    #[test]
    fn a_declared_run_environment_reaches_the_launch_context() {
        let store = scratch("declared-run-env");
        let secrets = adi_secrets::Secrets::with_config(store.config().clone());
        secrets
            .set(None, "SHARED", "from-secret", None)
            .expect("secret");

        let mut m = spec("process:claude");
        m.path = vec!["/opt/node22/bin".into()];
        m.env = [
            ("NODE_ENV".to_string(), "development".to_string()),
            // A declared value outranks an attached secret of the same name.
            ("SHARED".to_string(), "from-env".to_string()),
            // ...but PATH is never taken from here: it is assembled below.
            ("PATH".to_string(), "/nowhere".to_string()),
        ]
        .into_iter()
        .collect();
        m.secrets = vec![SecretAttachment {
            project: None,
            name: "SHARED".into(),
        }];
        let agent = store.save("solver", m).expect("save");
        let agent = store.get(&agent.name).expect("get").expect("present");

        let ctx = store.launch_context(&agent, None);
        let env: std::collections::BTreeMap<_, _> = ctx.run_env.into_iter().collect();
        assert_eq!(env.get("NODE_ENV").map(String::as_str), Some("development"));
        assert_eq!(env.get("SHARED").map(String::as_str), Some("from-env"));
        assert!(!env.contains_key("PATH"), "PATH must not come from [env]");

        let dirs: Vec<_> = std::env::split_paths(&ctx.run_path).collect();
        assert!(
            dirs.iter()
                .any(|d| d == std::path::Path::new("/opt/node22/bin")),
            "{}",
            ctx.run_path
        );
        assert!(
            !dirs.iter().any(|d| d == std::path::Path::new("/nowhere")),
            "{}",
            ctx.run_path
        );
    }

    #[test]
    fn an_empty_allowlist_injects_nothing_and_touches_no_key() {
        let store = scratch("empty-allowlist");
        assert!(attached_secret_env(store.config(), &[]).is_empty());
    }

    #[test]
    fn rename_validates_both_names_and_no_ops_on_self() {
        let store = scratch("rename-names");
        store.save("keep", spec("pty:claude")).expect("save");

        assert!(matches!(
            store.rename("keep", "../escape"),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(
            store.rename("a/b", "keep"),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(
            store.rename("ghost", "fresh"),
            Err(Error::NotFound(_))
        ));
        store
            .rename("keep", "keep")
            .expect("self rename is a no-op");
        assert!(store.get("keep").expect("still there").is_some());
    }

    /// Register a tool in `store`'s registry that answers `llm help` and nothing else — the
    /// convention a tool documents itself by. Returns its id, for an agent's `bin_tools`.
    fn tool_answering_llm_help(store: &Agents, name: &str, help: &str) -> String {
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = \"llm\" ] && [ \"$2\" = \"help\" ]; then\n\
             printf '%s' '{help}'\nelse\n  exit 1\nfi\n"
        );
        adi_tools::Tools::with_config(store.config.clone())
            .create_file(name, None, "sh", None, Some(script))
            .expect("register the tool")
            .id
    }

    fn agent_with_tools(
        store: &Agents,
        name: &str,
        backend: &str,
        tools: Vec<String>,
    ) -> StoredAgent {
        let mut manifest = spec(backend);
        manifest.arguments.system_prompt = Some("You are a careful operator.".into());
        manifest.bin_tools = tools;
        store.save(name, manifest).expect("save");
        store.get(name).expect("read back").expect("the agent")
    }

    /// The whole seam, end to end: a tool that answers `llm help` is asked at launch, and what it
    /// said arrives in the prompt the agent is launched with — behind the agent's own instructions,
    /// which are left exactly as written.
    #[test]
    fn a_launch_carries_the_enabled_tools_own_help() {
        let store = scratch("tool-help");
        let id = tool_answering_llm_help(&store, "greet", "Usage: greet <name>");
        let agent = agent_with_tools(&store, "helper", "harness:claude-sdk", vec![id]);

        let launched = store.decorated(&agent, store.config.root());
        let prompt = launched.manifest.arguments["system_prompt"]
            .as_str()
            .expect("a prompt");

        assert!(
            prompt.starts_with("You are a careful operator."),
            "{prompt}"
        );
        assert!(prompt.contains("# Your tools"), "{prompt}");
        assert!(prompt.contains("## greet"), "{prompt}");
        assert!(prompt.contains("Usage: greet <name>"), "{prompt}");

        // And it survives the typed decode every backend performs before building its argv — where
        // `deny_unknown_fields` would reject a key the backend has no field for. For claude-sdk
        // this value goes straight into `--append-system-prompt`.
        let typed: arguments::HarnessClaudeSdkArguments = launched
            .manifest
            .typed_arguments()
            .expect("typed arguments");
        assert!(
            typed
                .system_prompt
                .expect("a system prompt")
                .contains("Usage: greet <name>")
        );
        // Derived, never stored: the manifest on disk still holds only what the user wrote.
        let stored = store.get("helper").expect("read back").expect("the agent");
        assert_eq!(
            stored.manifest.arguments["system_prompt"].as_str(),
            Some("You are a careful operator.")
        );
    }

    /// The prompt every launch carries: where the run starts. An agent with no tools still gets it —
    /// having no tools says nothing about needing to know where it is — and it still gets no tool
    /// section, so the registry is never read.
    #[test]
    fn every_launch_states_where_the_run_starts() {
        let store = scratch("workspace-block");
        let agent = agent_with_tools(&store, "bare", "harness:claude-sdk", Vec::new());
        let prompt = store
            .decorated(&agent, store.config.root())
            .manifest
            .arguments["system_prompt"]
            .as_str()
            .expect("a prompt")
            .to_string();

        assert!(
            prompt.starts_with("You are a careful operator."),
            "{prompt}"
        );
        assert!(prompt.contains("# Where you are"), "{prompt}");
        assert!(
            prompt.contains(&store.config.root().display().to_string()),
            "{prompt}"
        );
        assert!(!prompt.contains("# Your tools"), "{prompt}");
    }

    /// The per-run directory is what the prompt states — not the manifest's, or the run is told one
    /// thing and started in another.
    #[test]
    fn the_prompt_states_the_directory_the_run_actually_gets() {
        let store = scratch("workspace-block-run-dir");
        let agent = agent_with_tools(&store, "passive", "harness:claude-sdk", Vec::new());
        let ctx = store.launch_context(&agent, Some("/targets/crescendo-ai"));

        assert_eq!(ctx.base_dir, std::path::Path::new("/targets/crescendo-ai"));
        assert!(
            ctx.agent.manifest.arguments["system_prompt"]
                .as_str()
                .expect("a prompt")
                .contains("/targets/crescendo-ai"),
            "{:?}",
            ctx.agent.manifest.arguments["system_prompt"]
        );
        // And the same directory is in the environment, for the scripts a run writes.
        assert!(
            ctx.run_env
                .iter()
                .any(|(k, v)| k == "ADI_WORKDIR" && v == "/targets/crescendo-ai"),
            "{:?}",
            ctx.run_env
        );
    }

    /// Write a conversation sidecar for `agent` by hand, as the harness would. `working_dir` is
    /// `None` for a conversation from before the field existed. Returns its id.
    fn seed_conversation(store: &Agents, agent: &str, working_dir: Option<&str>) -> String {
        let dir = store
            .config
            .module(SESSIONS_MODULE)
            .dir()
            .join("harness")
            .join(agent);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let conv_id = "1700000000000-0000";
        let mut meta = serde_json::json!({ "started_at": 1_700_000_000_000u64, "message": "go" });
        if let Some(d) = working_dir {
            meta["working_dir"] = serde_json::Value::String(d.to_string());
        }
        std::fs::write(dir.join(format!("{conv_id}.json")), meta.to_string()).expect("write meta");
        conv_id.to_string()
    }

    /// Seed a live run of `agent` under `subdir`: a PID file naming *this* process, which is exactly
    /// what a running run looks like to the counter.
    fn seed_live_run(store: &Agents, subdir: &str, agent: &str, run_id: &str) {
        let dir = store
            .config
            .module(SESSIONS_MODULE)
            .dir()
            .join(subdir)
            .join(agent);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join(format!("{run_id}.pid")),
            format!("{}\n", std::process::id()),
        )
        .expect("write pid");
    }

    /// Live runs are counted across agents and across backends — one number, whoever started them.
    #[test]
    fn the_run_count_spans_every_agent_and_backend() {
        let store = scratch("run-count");
        assert_eq!(store.running_count(), 0);

        seed_live_run(&store, "process", "recon", "0000000000001-0000");
        seed_live_run(&store, "process", "recon", "0000000000002-0000");
        seed_live_run(&store, "harness", "chatty", "0000000000003-0000");
        assert_eq!(store.running_count(), 3);

        // A finished run drops its PID file, and a stale one naming a dead process doesn't count.
        std::fs::write(
            store
                .config
                .module(SESSIONS_MODULE)
                .dir()
                .join("process/recon/0000000000004-0000.pid"),
            "4294967294\n",
        )
        .expect("write stale pid");
        assert_eq!(store.running_count(), 3);
    }

    /// The cap refuses a launch before anything is spawned — and `force` is the way past it. The
    /// forced launch still fails here (the `adi` harness has no provider configured in a scratch
    /// store), but it fails *at the backend*, which is the whole point: the cap no longer stopped it.
    #[test]
    fn the_run_cap_refuses_a_launch_and_force_gets_past_it() {
        let store = scratch("run-cap");
        store
            .save("solver", spec("harness:adi"))
            .expect("save an agent");
        store
            .set_limits(RunLimits {
                max_concurrent_runs: 2,
                ..RunLimits::default()
            })
            .expect("set the limit");

        seed_live_run(&store, "process", "other", "0000000000001-0000");
        assert!(
            store.run_in("solver", "go", None).is_ok()
                || matches!(store.run_in("solver", "go", None), Err(Error::NotRunnable(_))),
            "one live run of two is below the cap, so the launch reaches the backend"
        );

        seed_live_run(&store, "harness", "other", "0000000000002-0000");
        assert!(
            matches!(
                store.run_in("solver", "go", None),
                Err(Error::TooManyRunning {
                    project: None,
                    running: 2,
                    limit: 2
                })
            ),
            "at the cap the launch is refused, naming what is running and what is allowed"
        );
        assert!(
            !matches!(
                store.force_run_in("solver", "go", None),
                Err(Error::TooManyRunning { .. })
            ),
            "force is the human's way past the cap"
        );

        // Lifting the limit lets it through again.
        store
            .set_limits(RunLimits {
                max_concurrent_runs: 0,
                ..RunLimits::default()
            })
            .expect("lift the limit");
        assert!(!matches!(
            store.run_in("solver", "go", None),
            Err(Error::TooManyRunning { .. })
        ));
    }

    /// A project's own cap narrows the global one: its agents stop at it while the rest of the
    /// machine — still below the global cap — carries on launching.
    #[test]
    fn a_project_cap_binds_only_that_projects_agents() {
        let store = scratch("project-cap");
        let mut scoped = spec("harness:adi");
        scoped.project = Some("bugbounty".into());
        store.save("solver", scoped).expect("save a project agent");
        let mut other_project = spec("harness:adi");
        other_project.project = Some("mono".into());
        store
            .save("builder", other_project)
            .expect("save another project's agent");
        store.save("loose", spec("harness:adi")).expect("save");

        store
            .set_limits(RunLimits {
                max_concurrent_runs: 10,
                ..RunLimits::default()
            })
            .expect("a global cap well above the project one");
        store
            .set_project_limit("bugbounty", 1)
            .expect("set the project cap");

        // One live run of the project fills its cap…
        seed_live_run(&store, "harness", "solver", "0000000000001-0000");
        assert_eq!(store.run_load().in_project("bugbounty"), 1);
        assert!(
            matches!(
                store.run_in("solver", "go", None),
                Err(Error::TooManyRunning { project: Some(p), running: 1, limit: 1 }) if p == "bugbounty"
            ),
            "the refusal names the project whose cap is full"
        );
        // …and nobody else's: another project and an unfiled agent are only bound by the global cap.
        for name in ["builder", "loose"] {
            assert!(
                !matches!(
                    store.run_in(name, "go", None),
                    Err(Error::TooManyRunning { .. })
                ),
                "{name} is outside the capped project"
            );
        }
        // Force still gets through, and clearing the project's cap leaves only the global one.
        assert!(!matches!(
            store.force_run_in("solver", "go", None),
            Err(Error::TooManyRunning { .. })
        ));
        store
            .set_project_limit("bugbounty", 0)
            .expect("clear the project cap");
        assert!(!matches!(
            store.run_in("solver", "go", None),
            Err(Error::TooManyRunning { .. })
        ));
    }

    /// The global cap is a ceiling, not a default: a project allowed more than the machine is still
    /// stopped by the machine's number.
    #[test]
    fn the_global_cap_still_binds_a_project_allowed_more() {
        let store = scratch("ceiling");
        let mut scoped = spec("harness:adi");
        scoped.project = Some("bugbounty".into());
        store.save("solver", scoped).expect("save");
        store
            .set_limits(RunLimits {
                max_concurrent_runs: 1,
                ..RunLimits::default()
            })
            .expect("set the global cap");
        store
            .set_project_limit("bugbounty", 5)
            .expect("a roomier project cap");

        seed_live_run(&store, "process", "other", "0000000000001-0000");
        assert!(
            matches!(
                store.run_in("solver", "go", None),
                Err(Error::TooManyRunning {
                    project: None,
                    running: 1,
                    limit: 1
                })
            ),
            "the global cap is reported, since it is the one that is full"
        );
    }

    /// Nothing typed into a chat is lost to the cap: a message to an idle conversation queues
    /// instead of being refused, and starts when a slot frees.
    #[test]
    fn a_reply_queues_rather_than_being_refused_at_the_cap() {
        let store = scratch("reply-cap");
        store
            .save("chatty", spec("harness:claude-sdk"))
            .expect("save");
        let agent = store.get("chatty").expect("get").expect("present");
        let conv = seed_conversation(&store, "chatty", None);
        store
            .set_limits(RunLimits {
                max_concurrent_runs: 1,
                ..RunLimits::default()
            })
            .expect("set the limit");
        seed_live_run(&store, "process", "other", "0000000000001-0000");

        assert_eq!(
            store.reply("chatty", &conv, "and then?").expect("reply"),
            Sent::Queued { place: 1 },
            "a full cap queues the message rather than refusing it"
        );
        // And the platform does not start it by itself while the cap is still full, even though the
        // conversation is idle and the message is waiting.
        let sessions_dir = store.config.module(SESSIONS_MODULE).dir().to_path_buf();
        assert!(
            can_advance_in(&agent, &sessions_dir, &conv),
            "the queued message is otherwise ready to start"
        );
        assert!(
            !store.advance_queue(&agent, &conv),
            "an automatic start waits for a free slot"
        );
    }

    /// A conversation answers from the directory it started in, however the manifest resolves now.
    #[test]
    fn a_conversation_re_enters_the_directory_it_started_in() {
        let store = scratch("conv-pinned");
        let agent = agent_with_tools(&store, "passive", "harness:claude-sdk", Vec::new());
        let conv = seed_conversation(&store, "passive", Some("/targets/crescendo-ai"));
        assert_eq!(
            store.conversation_dir(&agent, &conv),
            std::path::Path::new("/targets/crescendo-ai")
        );
    }

    /// A conversation started before the directory was recorded ran in the store root. It has to
    /// keep answering from there — a fresh resolve would now hand a project agent its project
    /// directory, and the Claude session it resumes is keyed by the cwd it was established under.
    #[test]
    fn a_conversation_from_before_the_field_stays_in_the_store_root() {
        let store = scratch("conv-legacy");
        let mut manifest = spec("harness:claude-sdk");
        manifest.project = Some("bugbounty".into());
        store.save("legacy", manifest).expect("save");
        let agent = store.get("legacy").expect("read back").expect("the agent");
        // The project exists, so an unpinned resolve *would* move this agent's runs into it.
        let project = store.config.root().join("projects").join("bugbounty");
        std::fs::create_dir_all(&project).expect("mkdir");
        assert_eq!(
            workspace::resolve(&store.config, &agent.manifest, None),
            project
        );

        let conv = seed_conversation(&store, "legacy", None);
        assert_eq!(store.conversation_dir(&agent, &conv), store.config.root());
    }

    /// Codex takes its `system_prompt` as the opening user turn, so anything folded in would land
    /// there as a question. Its agents keep the prompt they were given — tools, location, and all.
    #[test]
    fn a_codex_agent_keeps_its_prompt_as_written() {
        let store = scratch("tool-help-codex");
        let id = tool_answering_llm_help(&store, "greet", "Usage: greet <name>");
        let agent = agent_with_tools(&store, "coder", "pty:codex", vec![id]);
        assert_eq!(store.decorated(&agent, store.config.root()), agent);
    }

    /// A tool ticked on but no longer registered (deleted, or archived out from under the agent)
    /// contributes nothing — being the only one, it leaves no tool section behind.
    #[test]
    fn an_unregistered_tool_adds_nothing() {
        let store = scratch("tool-help-ghost");
        let agent = agent_with_tools(
            &store,
            "haunted",
            "harness:claude-sdk",
            vec!["no-such-tool".into()],
        );
        let prompt = store
            .decorated(&agent, store.config.root())
            .manifest
            .arguments["system_prompt"]
            .as_str()
            .expect("a prompt")
            .to_string();
        assert!(!prompt.contains("# Your tools"), "{prompt}");
    }
}
