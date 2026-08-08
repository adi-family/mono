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
pub mod analytics;
pub mod arguments;
pub mod awaits;
mod backend;
mod backends;
mod error;
mod events;
mod launch;
mod limits;
mod memo;
pub mod progress;
pub mod review;
mod run;
pub mod runner;
pub mod store;
mod tool_help;
mod workspace;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

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

use agent::validate_name;
use runner::{RunEvent, RunSpec, Runner, Session, runner_for};
use store::{SessionRecord, SessionRef, SessionStore, assistant_turn, user_turn};

const AGENTS_MODULE: &str = "agents";
const SESSIONS_MODULE: &str = "sessions";
const MANIFEST_EXT: &str = "toml";

/// How long a stopped run is given to end itself before it is killed.
///
/// Long enough for a vendor CLI to finish the request it is blocked on and write out what it has;
/// short enough that a human who clicked Stop does not sit watching a run they have already ended.
/// A run that ignores the whole of it was never going to stop cooperatively.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// Serializes "is a turn already running?" with the send that follows it.
///
/// A session has one log slot, so one turn may be in flight at a time. The app server answers
/// requests concurrently and an open chat polls *two* endpoints a second — the run list and the
/// transcript, both of which advance the queue — so without this the check and the send could
/// interleave and start two children into the same slot, each clobbering the other's log. Turn
/// starts are rare and take milliseconds, so one gate costs nothing; reads never take it.
static TURN_GATE: Mutex<()> = Mutex::new(());

/// Hold the turn gate. A previous panic while holding it says nothing about what is on disk, so a
/// poisoned lock is taken anyway rather than propagating that panic into every later turn.
fn turn_gate() -> MutexGuard<'static, ()> {
    TURN_GATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An on-disk agent registry.
#[derive(Debug, Clone)]
pub struct Agents {
    config: Config,
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

    /// This store's sessions: the record, the queue, the transcript, and the log of every run.
    ///
    /// A path and no cached state, so it is taken per call rather than held — which is only
    /// affordable because opening one does nothing but keep the path. It used to also sweep the
    /// pre-flat layout (`<sessions>/<process|harness>/<agent>/`) forward on every open, since no one
    /// of the app, the CLI, and a trigger's child can be the one that does it once. That sweep is
    /// gone: it is called from the listing path, once per agent and again per idle run, so a single
    /// `/api/agents/runs/all` paid for it four hundred times over — and it could never finish
    /// anyway, because a legacy directory holding a session the new layout already has is left
    /// standing on purpose, and so is rescanned for ever.
    fn sessions(&self) -> SessionStore {
        SessionStore::new(self.config.module(SESSIONS_MODULE).dir())
    }

    /// The runner for this agent's backend, or the honest refusal.
    ///
    /// # Errors
    /// [`Error::NotRunnable`] when nothing here runs that backend.
    fn runner_of(agent: &StoredAgent) -> Result<Box<dyn Runner>> {
        runner_for(&agent.manifest.backend)
            .ok_or_else(|| Error::NotRunnable(agent.manifest.backend.to_string()))
    }

    /// Whether this session's work is still in flight, asked of whichever runner started it.
    ///
    /// The record's own backend, not the agent's current one: re-pointing an agent at another
    /// engine must not make its existing runs unreadable, which is the whole reason the backend is
    /// a field of the session rather than a directory it lives in.
    fn session_is_alive(store: &SessionStore, record: &SessionRecord) -> bool {
        runner_for(&record.backend)
            .is_some_and(|runner| runner.is_alive(&store.session(&record.agent, &record.id)))
    }

    /// What is running right now, totalled and divided between projects — one walk of the session
    /// store, weighed against [`Self::limits`].
    ///
    /// Every agent's sessions are counted, including those of an agent whose definition has since
    /// been deleted: what bounds the machine is the processes that exist, not the manifests that
    /// explain them. The agent list is only read when something is actually running (it is needed
    /// solely to attribute a run to a project), so the common case — an idle machine asking whether
    /// it may start something — costs a directory scan and nothing else.
    #[must_use]
    pub fn run_load(&self) -> RunLoad {
        let store = self.sessions();
        let mut by_agent: BTreeMap<String, usize> = BTreeMap::new();
        for agent in store.agents() {
            let live = store
                .list(&agent)
                .iter()
                .filter(|record| Self::session_is_alive(&store, record))
                .count();
            if live > 0 {
                by_agent.insert(agent, live);
            }
        }
        if by_agent.is_empty() {
            return RunLoad::default();
        }
        RunLoad::new(
            &by_agent,
            self.list()
                .unwrap_or_default()
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

    /// The one launch path: resolve the agent, weigh the cap unless `force`, open a session, send,
    /// announce.
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
        let runner = Self::runner_of(&agent)?;
        // One pane per agent, so a second Run is not a launch — it would type this task into the
        // session already open. Say so instead.
        if runner.as_terminal().is_some() && self.is_running(&agent) {
            return Err(Error::AlreadyRunning(name.to_string()));
        }

        let spec = self.launch_spec(&agent, working_dir);
        // Fail before anything is written down: a mistyped argument should leave no session behind.
        runner.check(&spec)?;
        let store = self.sessions();
        let record = store.create(
            &agent.name,
            agent.manifest.backend.clone(),
            &spec.cwd,
            message,
        )?;
        let session = store.session(&agent.name, &record.id);
        // The opening question, recorded as a turn so the run reads as a conversation from its
        // first line. Not for a terminal: its launch message is deliberately never typed (the TUI
        // is still drawing its first frame), so recording it as asked would be a lie.
        if runner.as_terminal().is_none() {
            store.append_turn(&agent.name, &record.id, user_turn(message))?;
        }
        runner.send(&spec, &session, message)?;
        // Only now, with this run's own files in place: pruning first would count it as one of the
        // old ones. A run somebody else is still watching is never swept.
        store.prune_old(&agent.name, |record| {
            Self::session_is_alive(&store, record)
        });

        let launch = launch_of(&agent, runner.as_ref(), &session);
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
        let runner = Self::runner_of(&agent)?;
        if !answerable(runner.as_ref()) {
            return Err(Error::Unsupported(format!(
                "backend {} isn't answerable — only a backend that continues the same thread keeps \
                 conversations you can reply to",
                agent.manifest.backend
            )));
        }
        let store = self.sessions();
        let record = store
            .get(name, conv_id)
            .ok_or_else(|| Error::NotFound(format!("{name}: no conversation {conv_id}")))?;
        let may_start = !self.at_capacity_for(&agent);

        let _gate = turn_gate();
        let session = store.session(name, conv_id);
        // Still answering, or no slot free: the message keeps its place and is asked when one of
        // those changes. Nothing typed into a chat is ever refused.
        if !may_start || runner.is_alive(&session) {
            let place = store.enqueue(name, conv_id, message)?;
            return Ok(Sent::Queued { place });
        }
        // Idle, but with a queue no read has drained yet: join the back of the line and start its
        // head instead, so messages are always answered in the order they were typed.
        let next = if store.queue_len(name, conv_id) > 0 {
            store.enqueue(name, conv_id, message)?;
            store
                .dequeue(name, conv_id)?
                .unwrap_or_else(|| message.to_string())
        } else {
            message.to_string()
        };

        let launch = self.start_turn(&agent, &store, runner.as_ref(), &record, &next)?;
        self.emit(
            "adi.agents.run.started",
            &AgentRunStarted::of(name, &next, &launch),
        );
        Ok(Sent::Started(launch))
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
        let Some(runner) = runner_for(&agent.manifest.backend) else {
            return Vec::new();
        };
        let store = self.sessions();
        if store.get(&agent.name, conv_id).is_none() {
            return Vec::new();
        }
        let session = store.session(&agent.name, conv_id);
        let running = runner.is_alive(&session);
        let live = live_content(runner.as_ref(), &session, running);
        // Commit the answer before showing it, so what a reader sees settle is the same text that
        // is on disk a moment later. A no-op while a turn is in flight, or once it has landed.
        if !running {
            settle(&store, &agent.name, conv_id, &live);
        }
        store.transcript(&agent.name, conv_id, Some(live), running)
    }

    /// Where one conversation ran — the directory pinned to it at creation and re-used by every turn
    /// since. `None` for a session that does not exist; the path may since have been deleted, which
    /// is the caller's to check.
    #[must_use]
    pub fn run_cwd(&self, agent: &StoredAgent, conv_id: &str) -> Option<PathBuf> {
        self.sessions().get(&agent.name, conv_id).map(|r| r.cwd)
    }

    /// Write the review dossier for one conversation beside its log, and return the brief that
    /// points a reviewing agent at it.
    ///
    /// This gathers and writes; it launches nothing. **Who** reviews is not a question this crate
    /// can answer — the app's root agent, a reviewer an operator picked, or the same agent looking
    /// at itself are all reasonable, and each caller knows which it means.
    ///
    /// The dossier lands in the session's own `<id>.review.md`, the sidecar namespace beside the log
    /// (`docs/sessions.md`), so it is deleted with the session it describes and never outlives it.
    ///
    /// # Errors
    /// [`Error::NotFound`] for an unknown conversation, [`Error::Unsupported`] for one that has not
    /// said anything yet (there is nothing to review, and an empty dossier would waste a run), and
    /// [`Error::Io`] if the file cannot be written.
    pub fn review(
        &self,
        agent: &StoredAgent,
        conv_id: &str,
        opts: review::Options,
    ) -> Result<review::Review> {
        let store = self.sessions();
        let Some(record) = store.get(&agent.name, conv_id) else {
            return Err(Error::NotFound(format!("{}/{conv_id}", agent.name)));
        };
        let turns = self.transcript(agent, conv_id);
        // Queued messages have been typed, not asked. A conversation that is only a queue has cost
        // nothing and done nothing, so there is no workflow in it to review.
        if turns.iter().all(|t| t.queued) {
            return Err(Error::Unsupported(
                "This conversation hasn't said anything yet — there's nothing to review.".to_string(),
            ));
        }

        let report = analytics::analyze(&turns, analytics::Options::default());

        // The agent's recent past, newest first. One conversation cannot tell a run that went badly
        // from a tool that always does, which is the whole reason this costs a second read.
        let mut history = review::History::default();
        let sessions = store.list(&agent.name);
        history.sessions_total = sessions.len();
        for record in sessions.iter().take(opts.history_sessions) {
            history.fold(record, &store.turns(&agent.name, &record.id));
        }
        history.settle();

        let (tools_on, tools_off) = self.tool_split(agent);

        let evidence = review::Evidence {
            agent,
            run_id: conv_id,
            record: &record,
            turns: &turns,
            report: &report,
            history: &history,
            tools_on: &tools_on,
            tools_off: &tools_off,
        };

        let dir = store.agent_dir(&agent.name);
        std::fs::create_dir_all(&dir).map_err(Error::Io)?;
        let path = dir.join(format!("{conv_id}.review.md"));
        std::fs::write(&path, review::document(&evidence, opts)).map_err(Error::Io)?;

        let brief = review::brief(&evidence, &path);
        Ok(review::Review { path, brief })
    }

    /// The tool store split by what this agent was given: the tools on its PATH, and the ones it was
    /// not. The second half is what lets a reviewer say "there is already a tool for that".
    ///
    /// System tools are on every agent's PATH whether or not they are listed in `bin_tools`
    /// (`adi_tools::Tools::sync_agent_bin`), so they count as enabled here regardless — reporting one
    /// as available-but-off would send a reviewer to switch on something already there.
    fn tool_split(&self, agent: &StoredAgent) -> (Vec<(String, String)>, Vec<(String, String)>) {
        let tools = adi_tools::Tools::with_config(self.config.clone());
        let Ok(all) = tools.list() else {
            return (Vec::new(), Vec::new());
        };
        let (mut on, mut off) = (Vec::new(), Vec::new());
        for tool in all.into_iter().filter(|t| !t.is_archived()) {
            let enabled = tool.is_system() || agent.manifest.bin_tools.contains(&tool.id);
            let entry = (
                tool.manifest.name.clone(),
                tool.manifest.description.clone().unwrap_or_default(),
            );
            if enabled { on.push(entry) } else { off.push(entry) }
        }
        on.sort();
        off.sort();
        (on, off)
    }

    /// Drop one message from a conversation's queue by its position, for something you have thought
    /// better of before it was ever asked. Returns whether there was one there to drop.
    ///
    /// # Errors
    /// Returns name validation errors.
    pub fn unqueue(&self, name: &str, conv_id: &str, index: usize) -> Result<bool> {
        validate_name(name)?;
        if self.get(name)?.is_none() {
            return Ok(false);
        }
        // Gated: this is a read-modify-write of the same file a starting turn pops from, and an
        // ungated pair could write back an entry that has just been asked.
        let _gate = turn_gate();
        self.sessions().unqueue(name, conv_id, index)
    }

    /// Start a conversation's next queued message, if it is idle and one is waiting; reports whether
    /// a turn started. The cheap gate comes first so an idle poll — which is nearly every poll —
    /// costs one `exists` rather than the tool sync a launch context pays for.
    ///
    /// This is the platform starting a run by itself, so the [run cap](RunLimits) binds absolutely:
    /// at capacity the message stays at the head of its queue and the next poll asks again.
    fn advance_queue(&self, agent: &StoredAgent, conv_id: &str) -> bool {
        let store = self.sessions();
        // The cheap gate, taken without the lock and re-taken under it: nothing waiting is nearly
        // every poll, and it must not cost the tool sync a launch spec pays for.
        if store.queue_len(&agent.name, conv_id) == 0 {
            return false;
        }
        let Some(runner) = runner_for(&agent.manifest.backend) else {
            return false;
        };
        if !answerable(runner.as_ref()) || self.at_capacity_for(agent) {
            return false;
        }
        let Some(record) = store.get(&agent.name, conv_id) else {
            return false;
        };

        let _gate = turn_gate();
        // Re-decided here, where only one poller can be holding the gate.
        if runner.is_alive(&store.session(&agent.name, conv_id)) {
            return false;
        }
        // Dropped from the queue *before* the turn starts: a message that fails to launch has still
        // had its turn, and leaving it at the head would retry it on every poll for ever.
        let Ok(Some(message)) = store.dequeue(&agent.name, conv_id) else {
            return false;
        };
        let Ok(launch) = self.start_turn(agent, &store, runner.as_ref(), &record, &message) else {
            return false;
        };
        self.emit(
            "adi.agents.run.started",
            &AgentRunStarted::of(&agent.name, &message, &launch),
        );
        true
    }

    /// Ask `message` in an existing session: settle whatever the last turn left behind, record the
    /// question, and send it.
    ///
    /// The session's own directory is re-used verbatim — an engine's session store is keyed by the
    /// directory it ran in, so re-resolving on turn five would make the thread unresumable and put
    /// the files earlier turns wrote out of reach.
    fn start_turn(
        &self,
        agent: &StoredAgent,
        store: &SessionStore,
        runner: &dyn Runner,
        record: &SessionRecord,
        message: &str,
    ) -> Result<Launch> {
        let session = store.session(&agent.name, &record.id);
        // Commit the previous turn's answer before the next question goes in, so the transcript
        // stays a clean question/answer sequence rather than two questions in a row. Nothing is in
        // flight here — every caller has taken the turn gate and found the session idle — so that
        // answer is whole.
        settle(
            store,
            &agent.name,
            &record.id,
            &live_content(runner, &session, false),
        );

        let spec = self.spec_in(agent, session_dir(&self.config, record));
        // Checked before the question is written down, so a spec this engine cannot run leaves no
        // dangling unanswered turn in the transcript.
        runner.check(&spec)?;
        store.append_turn(&agent.name, &record.id, user_turn(message))?;
        runner.send(&spec, &session, message)?;
        Ok(launch_of(agent, runner, &session))
    }

    /// Run one turn of a `harness:adi` conversation: read its transcript, call the configured model
    /// provider, and return the answer text. Invoked by the `adi-mono harness-turn` child that an
    /// `adi` turn spawns; that child prints the returned text, which the conversation folds into the
    /// transcript as the assistant's answer.
    ///
    /// This blocks on the provider HTTP call, so it must run in a sync context (the CLI child), never
    /// inside the app server's async runtime.
    ///
    /// The loop's system prompt is **not** assembled here. The runner composed it — the agent's own
    /// instructions, its location, its tools' help — and exported it to this child, which reads it
    /// back in `adi_loop::run_turn`. Re-deriving it here would be a second composition path for the
    /// same prompt, and two paths that have to agree eventually don't.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`] for an unknown agent, [`Error::Unsupported`] for a backend with
    /// no loop of its own to run, or argument / provider-configuration / HTTP / decoding errors.
    pub fn run_adi_turn(
        &self,
        agent_name: &str,
        conv_id: &str,
        sink: crate::backends::adi_events::Sink<'_>,
    ) -> Result<String> {
        let agent = self
            .get(agent_name)?
            .ok_or_else(|| Error::NotFound(agent_name.to_string()))?;
        // The one verb no runner has: every other engine answers a turn in a child process the
        // runner spawned, while the `adi` loop *is* that child. So it stays a direct call, and the
        // backend check that used to be a dispatch match is now the two lines it always was.
        if agent.manifest.backend != Backend::HarnessAdi {
            return Err(Error::Unsupported(format!(
                "backend {} has no adi loop to run",
                agent.manifest.backend
            )));
        }
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        backends::harness::run_adi_turn(&agent, &sessions_dir, conv_id, sink)
    }

    /// Serve this conversation's ADI tools to a Claude engine over MCP, until the engine's CLI
    /// closes the pipe.
    ///
    /// Invoked by the `adi-mono mcp` child the runner registers on every Claude-engine run (see
    /// [`runner::detached`]). `cwd` is the run's own directory: this process is a *grandchild* of
    /// the runner, so the directory it starts in is the CLI's business rather than a thing to
    /// resolve an agent's relative paths against.
    ///
    /// Reads stdin and writes stdout, which are the transport — so nothing else may print to them
    /// for the lifetime of the call.
    ///
    /// # Errors
    /// [`Error::Process`] if the transport fails. A tool that fails is not an error: its message
    /// travels back as the call's result, exactly as it does in the adi loop.
    pub fn serve_mcp(&self, agent: &str, conv: &str, cwd: &std::path::Path) -> Result<()> {
        let agent_dir = self.sessions().agent_dir(agent);
        // The conversation's shell keeps its state in sidecars of this directory, and writes them
        // from inside the command it runs — so a missing directory is not an error anybody sees, it
        // is a `Bash` that reports a redirection failure instead of the output the model asked for.
        // The store creates it on the write paths; this entry point is reached by a *child of the
        // engine's CLI* and cannot assume any of them ran first.
        std::fs::create_dir_all(&agent_dir)?;
        let stdin = std::io::stdin();
        backends::mcp::serve(
            agent,
            conv,
            cwd,
            &agent_dir,
            stdin.lock(),
            std::io::stdout().lock(),
        )
    }


    /// The spec a *fresh* run starts with. `run_dir` is this launch's own working directory, for a
    /// caller pointing one agent at a different target each run; `None` leaves the manifest and the
    /// agent's project to decide (see [`workspace::resolve`] for the full precedence).
    fn launch_spec(&self, agent: &StoredAgent, run_dir: Option<&str>) -> RunSpec {
        self.spec_in(
            agent,
            workspace::resolve(&self.config, &agent.manifest, run_dir),
        )
    }

    /// Everything a run needs, resolved and already on disk: where it starts, the `PATH` it
    /// resolves commands on, the environment it is launched with, its engine's configuration, and
    /// its tools' own help as data.
    ///
    /// **This is the write path.** Assembling a spec syncs the agent's `.bin` of enabled tools and
    /// asks each of them to describe itself, so it is never built to answer a read — that is what
    /// the cheap gates in front of it exist for.
    ///
    /// Nothing here writes into the agent's prompt. The tools travel as [`ToolHelp`](adi_tools::ToolHelp)
    /// and the prompt travels as the user wrote it, because *where* help belongs — an appended
    /// system prompt, a system message, or nowhere at all for an engine whose "system prompt" is
    /// really its opening user turn — is a fact about the engine, and the runner is the only layer
    /// that knows it.
    fn spec_in(&self, agent: &StoredAgent, cwd: PathBuf) -> RunSpec {
        let tools = adi_tools::Tools::with_config(self.config.clone());
        // Best-effort: a sync failure (or no tools) just means no extra bin on PATH, never a blocked run.
        let bin_dir = tools
            .sync_agent_bin(&agent.name, &agent.manifest.bin_tools)
            .ok();
        // Allowlist only — nothing pulled in from a scope just for existing. Resolved against this
        // store's Config (test stores stay isolated). Best-effort: a missing/undecryptable secret is skipped.
        let mut env = attached_secret_env(&self.config, &agent.manifest.secrets);
        // Point the run at its scope's shared database (a project agent gets that project's), so
        // `adi-db` in its shell and `import … from "@adi/db"` in its `ts` code both resolve without
        // the agent being told where the file is. Secrets are listed first, so a secret named
        // `ADI_DB` would be overridden here rather than silently redirecting the run's database.
        env.extend(self.config.db_env(agent.manifest.project.as_deref()));
        // Where the run starts, what it is called, and what it is scoped to. A run that has to be
        // *told* its directory in prose gets it wrong; a script it writes can't be told at all.
        env.extend(workspace::env(&self.config, agent, &cwd));
        // The agent's own declared vars last, so an explicit `[env]` entry wins over a secret or
        // the database pointer it collides with — it is the most specific statement about this
        // agent there is. `PATH` is dropped rather than honoured: it is built from the manifest's
        // `path` below and applied after every var, so a `PATH` here would be silently overridden.
        env.extend(
            agent
                .manifest
                .env
                .iter()
                .filter(|(key, _)| key.as_str() != "PATH")
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        // Stated, not merely exported: `ADI_WORKDIR` is there for scripts, but the agent itself
        // reads prose, and one that has to infer its directory infers it wrong.
        let workspace_note =
            Some(workspace::block(&self.config, agent, &cwd)).filter(|note| !note.trim().is_empty());
        RunSpec {
            cwd,
            path: launch::run_path(bin_dir.as_deref(), &agent.manifest.path),
            env,
            arguments: agent.manifest.arguments_value(),
            // Asked once per launch, not stored: enabling a tool, editing its help, or upgrading
            // the CLI underneath it shows up on the next run without anyone rewriting a prompt.
            tools: if agent.manifest.bin_tools.is_empty() {
                Vec::new()
            } else {
                tools.help_for(&agent.manifest.bin_tools)
            },
            system_prompt: agent.manifest.system_prompt(),
            workspace_note,
        }
    }

    #[must_use]
    pub fn is_running(&self, agent: &StoredAgent) -> bool {
        let store = self.sessions();
        store
            .list(&agent.name)
            .iter()
            .any(|record| Self::session_is_alive(&store, record))
    }

    /// A read-only live snapshot of an agent for the live view: a pty screen capture for interactive
    /// backends, or the latest run's log tail for the headless backends.
    #[must_use]
    pub fn peek(&self, agent: &StoredAgent) -> Peek {
        let store = self.sessions();
        // Newest first, so this is the run somebody watching the agent means. A terminal has no run
        // list at all — its pane is the agent's, whichever session opened it — so `peek_run` finds
        // it from the name and the id it is handed does not matter.
        let latest = store
            .list(&agent.name)
            .first()
            .map(|record| record.id.clone())
            .unwrap_or_default();
        self.peek_run(agent, &latest)
    }

    /// The run history of a headless agent, newest first (empty for interactive backends, whose
    /// live session is their only "run").
    ///
    /// Like [`Self::transcript`], listing advances any conversation whose answer has landed with
    /// something still queued behind it — so a queue keeps moving while you are reading some *other*
    /// chat, not only the one you have open.
    #[must_use]
    pub fn runs(&self, agent: &StoredAgent) -> Vec<RunInfo> {
        let Some(runner) = runner_for(&agent.manifest.backend) else {
            return Vec::new();
        };
        // An interactive backend's live pane *is* its run: there is no history to page through.
        if runner.as_terminal().is_some() {
            return Vec::new();
        }
        let store = self.sessions();
        let runs = Self::list_runs(&store, agent, runner.as_ref());
        // Which sessions have anything waiting, in one question rather than one per run. Nothing is
        // waiting in nearly every poll, and asking each run separately made the *empty* answer the
        // expensive one.
        let waiting = store.sessions_with_queue(&agent.name);
        let idle: Vec<String> = runs
            .iter()
            .filter(|r| !r.running && waiting.contains(&r.run_id))
            .map(|r| r.run_id.clone())
            .collect();
        let advanced = idle.iter().fold(false, |any, conv_id| {
            self.advance_queue(agent, conv_id) || any
        });
        // Only re-read when something actually started — the answer must not report a conversation
        // as idle in the very breath it started its next turn.
        if advanced {
            return Self::list_runs(&store, agent, runner.as_ref());
        }
        runs
    }

    /// One agent's sessions as run history, newest first — the store's records with each one's
    /// liveness asked of the runner.
    fn list_runs(store: &SessionStore, agent: &StoredAgent, runner: &dyn Runner) -> Vec<RunInfo> {
        store
            .list(&agent.name)
            .into_iter()
            .map(|record| RunInfo {
                // From the record just listed: liveness reads the runner's state slot, which is a
                // column that row already carried.
                running: runner.is_alive(&store.session_as_listed(&record)),
                run_id: record.id,
                started_at: record.started_at,
                last_activity: record.last_activity,
                message: record.message,
                hidden: record.hidden,
            })
            .collect()
    }

    /// A read-only snapshot of one specific run of a headless agent (or the pty screen, for an
    /// interactive backend, where `run_id` is ignored).
    #[must_use]
    pub fn peek_run(&self, agent: &StoredAgent, run_id: &str) -> Peek {
        let Some(runner) = runner_for(&agent.manifest.backend) else {
            return empty_peek();
        };
        let store = self.sessions();
        let session = store.session(&agent.name, run_id);
        let running = runner.is_alive(&session);
        if let Some(terminal) = runner.as_terminal() {
            return Peek {
                running,
                output: terminal.capture(&session).unwrap_or_default(),
                // A pane has no external attach command; it is viewed only in the control panel.
                attach: String::new(),
                interactive: true,
            };
        }
        let log = session.log_path();
        Peek {
            running,
            output: backends::detached::tail_of(log, run::MAX_LOG_TAIL).unwrap_or_default(),
            attach: format!("tail -f {}", log.display()),
            interactive: false,
        }
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
        let Some(runner) = runner_for(&agent.manifest.backend) else {
            return Ok(false);
        };
        let store = self.sessions();
        let stopped = runner
            .stop(&store.session(name, run_id), STOP_GRACE)?
            .was_running;
        // Whatever was waiting behind this answer was written expecting it, so it goes with it
        // rather than marching on into a conversation somebody has just interrupted.
        store.clear_queue(name, run_id)?;
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
        let store = self.sessions();
        if store.get(name, run_id).is_none() {
            return Ok(false);
        }
        // Stop it first. The store cannot signal anything — that is the runner's half — so a child
        // still running here would outlive its own log and write into a slot nothing is reading.
        if let Some(runner) = runner_for(&agent.manifest.backend) {
            runner.stop(&store.session(name, run_id), STOP_GRACE)?;
        }
        let deleted = store.delete(name, run_id)?;
        if deleted {
            // A wake registered against a conversation that no longer exists has nowhere to land, so
            // it goes with it rather than firing into a `NotFound` a week from now. How many there
            // were is the caller's business elsewhere; here it is tidying.
            let _ = awaits::Awaits::with_config(self.config.clone())
                .forget_conversation(name, run_id);
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
        if self.get(name)?.is_none() {
            return Ok(false);
        }
        self.sessions().set_hidden(name, run_id, hidden)
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
        let Some(runner) = runner_for(&agent.manifest.backend) else {
            return Ok(false);
        };
        // Every live session of the agent, not only the ones a run list would show: a terminal
        // keeps no history, and stopping "the agent" has always meant closing its pane too.
        let store = self.sessions();
        let mut stopped = false;
        for record in store.list(name) {
            if runner
                .stop(&store.session(name, &record.id), STOP_GRACE)?
                .was_running
            {
                store.clear_queue(name, &record.id)?;
                stopped = true;
            }
        }
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
            // Nothing left to wake: every await this agent's conversations registered goes too.
            let _ = awaits::Awaits::with_config(self.config.clone()).forget_agent(name);
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

/// Whether this runner keeps a thread a reply can continue — a live pane is typed into, not replied
/// to, however resumable its session is. The same rule [`capabilities`] reports as `answerable`.
fn answerable(runner: &dyn Runner) -> bool {
    runner.resumes() && runner.as_terminal().is_none()
}

/// Where a turn of this session runs: the directory it was opened in, re-entered by every later
/// turn so a thread stays in one place for its whole life.
///
/// A record with no directory — one whose sidecar was lost, or a session from before the field
/// existed — falls back to the store root, which is where every run started back then. Not a fresh
/// resolve: that would now hand a project-scoped agent its project directory and strand the threads
/// it already has.
fn session_dir(config: &Config, record: &SessionRecord) -> PathBuf {
    if record.cwd.as_os_str().is_empty() {
        config.root().to_path_buf()
    } else {
        record.cwd.clone()
    }
}

/// This session's events, folded into one turn's content: its timeline in order, its answer, and
/// whatever telemetry the engine closed with.
///
/// Read from the top (`None` cursor) rather than incrementally, because what a reader wants is the
/// whole turn rather than what changed since it last asked. Memoized on the log, so an open chat
/// polling twice a second does not re-parse a finished turn on every tick.
///
/// `running` only keys the memo — a runner reads a live log differently from a finished one, and
/// the two answers must not be served for each other.
fn live_content(runner: &dyn Runner, session: &SessionRef<'_>, running: bool) -> TurnContent {
    let content = memo::folded_events(session.log_path(), !running, || {
        let Ok(batch) = runner.events(session, None) else {
            return TurnContent::default();
        };
        let mut content = TurnContent::default();
        for event in batch.events {
            match event {
                RunEvent::Step(step) => content.steps.push(step),
                RunEvent::Answer { text } => content.text = text,
                RunEvent::Metrics(metrics) => content.metrics = Some(metrics),
                // The turn ending is not part of what it said; liveness is asked of the runner.
                RunEvent::Finished { .. } => {}
            }
        }
        content
    });
    (*content).clone()
}

/// Commit `content` as this session's answer, if the last thing said was a question.
///
/// The lazy clock the whole conversation machinery runs on: there is no reaper, so a finished turn
/// is folded into the transcript by the next read of it. A no-op when the last turn is already an
/// answer, which is what makes it safe to call before every read and every send.
///
/// The answer is dated **when the engine finished it**, not when this ran. Those are the same
/// moment only when somebody happened to be watching; an agent that answered overnight settles the
/// first time its chat is opened, and stamping that instant would have opening a conversation
/// backdate to nothing and re-date it to now — moving it to the top of every listing sorted by when
/// it last spoke, for no other reason than that it was read.
fn settle(store: &SessionStore, agent: &str, id: &str, content: &TurnContent) {
    let turns = store.turns(agent, id);
    let Some(question) = turns.last().filter(|turn| turn.role == store::ROLE_USER) else {
        return;
    };
    let mut turn = assistant_turn(content);
    if let Some(finished) = finished_at(&store.log_path(agent, id)) {
        // Never before the question it answers, and never ahead of the clock reading it: a log
        // stamped by a machine whose time has since moved must not pin a chat to the top for ever.
        let now = turn.at;
        turn.at = finished.max(question.at).min(now);
    }
    let _ = store.append_turn(agent, id, turn);
}

/// When the engine last wrote to a session's log, as unix millis — how a turn that nobody watched
/// end says when it ended.
///
/// The log is the engine's own output, recreated for each turn and appended to until it stops, so
/// its mtime is the last moment this turn produced anything. `None` when there is no log or the
/// platform reports no mtime, which reads as "no better answer than now".
fn finished_at(log: &std::path::Path) -> Option<u64> {
    let modified = std::fs::metadata(log).and_then(|meta| meta.modified()).ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    u64::try_from(since.as_millis()).ok()
}

/// The launch handle a caller gets back: the pane a terminal opened, or the child a headless runner
/// started.
///
/// `command` is the engine rather than the argv it built. The exact command line is the runner's
/// business now and no longer crosses this boundary — which is the point of the split, and costs a
/// caller only a hint it printed for a human.
fn launch_of(agent: &StoredAgent, runner: &dyn Runner, session: &SessionRef<'_>) -> Launch {
    let command = agent.manifest.backend.to_string();
    if runner.as_terminal().is_some() {
        return Launch::Pty {
            command,
            session: state_str(session, "session")
                .unwrap_or_else(|| run::session_name(&agent.name)),
        };
    }
    Launch::Process {
        command,
        // A runner whose engine lives elsewhere records no pid, and there is no local process to
        // name: `0` says exactly that, and the run is still addressed by its id.
        pid: state_str(session, "pid")
            .and_then(|pid| pid.parse().ok())
            .unwrap_or(0),
        log: session.log_path().to_path_buf(),
        run_id: session.id().to_string(),
    }
}

/// One conventional field of a runner's state slot, as a string.
///
/// The slot is the runner's own space and this is the one place anything above reads it — for the
/// two handles a launch has always reported: the pane a terminal opened and the pid a local child
/// got. A slot with neither reads as absent rather than as an error.
fn state_str(session: &SessionRef<'_>, key: &str) -> Option<String> {
    let value = session.state()?;
    let field = value.get(key)?;
    field
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
        .or_else(|| field.as_u64().map(|n| n.to_string()))
}

/// The snapshot of a backend nothing here runs: nothing to show, nothing to attach to.
fn empty_peek() -> Peek {
    Peek {
        running: false,
        output: String::new(),
        attach: String::new(),
        interactive: false,
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

        let spec = store.launch_spec(&agent, None);
        let env: std::collections::BTreeMap<_, _> = spec.env.into_iter().collect();
        assert_eq!(env.get("NODE_ENV").map(String::as_str), Some("development"));
        assert_eq!(env.get("SHARED").map(String::as_str), Some("from-env"));
        assert!(!env.contains_key("PATH"), "PATH must not come from [env]");

        let dirs: Vec<_> = std::env::split_paths(&spec.path).collect();
        assert!(
            dirs.iter()
                .any(|d| d == std::path::Path::new("/opt/node22/bin")),
            "{}",
            spec.path
        );
        assert!(
            !dirs.iter().any(|d| d == std::path::Path::new("/nowhere")),
            "{}",
            spec.path
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

    /// The store's half of the seam, end to end: a tool that answers `llm help` is really asked, and
    /// what it said reaches the spec as *data*. Where that data lands in a prompt is the runner's
    /// half — see `runner::detached`'s `tool_help_reaches_claudes_system_prompt_and_never_codexs`.
    ///
    /// Split deliberately. The store used to fold the help into `manifest.arguments.system_prompt`
    /// and then re-validate, because a backend with `deny_unknown_fields` would reject a key it had
    /// no field for. The runner appends to argv instead and never touches the stored arguments, so
    /// that whole failure mode — and the fallback that swallowed it — is gone rather than moved.
    #[test]
    fn a_launch_carries_the_enabled_tools_own_help() {
        let store = scratch("tool-help");
        let id = tool_answering_llm_help(&store, "greet", "Usage: greet <name>");
        let agent = agent_with_tools(&store, "helper", "harness:claude-sdk", vec![id]);

        let spec = store.spec_in(&agent, store.config.root().to_path_buf());

        assert_eq!(
            spec.system_prompt.as_deref(),
            Some("You are a careful operator."),
            "the agent's own instructions travel unmodified"
        );
        let help = spec.tools.iter().find(|t| t.name == "greet").expect("greet");
        assert!(
            help.help
                .as_deref()
                .is_some_and(|h| h.contains("Usage: greet <name>")),
            "{help:?}"
        );

        // Derived, never stored: the manifest on disk still holds only what the user wrote.
        let stored = store.get("helper").expect("read back").expect("the agent");
        assert_eq!(
            stored.manifest.arguments["system_prompt"].as_str(),
            Some("You are a careful operator.")
        );
    }

    /// Every spec states where the run starts. An agent with no tools still gets it — having no
    /// tools says nothing about needing to know where it is — and it still gets no tool section, so
    /// the registry is never read.
    #[test]
    fn every_launch_states_where_the_run_starts() {
        let store = scratch("workspace-block");
        let agent = agent_with_tools(&store, "bare", "harness:claude-sdk", Vec::new());

        let spec = store.spec_in(&agent, store.config.root().to_path_buf());
        let note = spec.workspace_note.expect("the location is stated");

        assert!(note.contains("# Where you are"), "{note}");
        assert!(
            note.contains(&store.config.root().display().to_string()),
            "{note}"
        );
        assert!(spec.tools.is_empty(), "{:?}", spec.tools);
    }

    /// The per-run directory reaches the spec — as the directory the run is *started* in, and in
    /// the environment, for the scripts a run writes.
    #[test]
    fn the_spec_carries_the_directory_the_run_actually_gets() {
        let store = scratch("workspace-block-run-dir");
        let agent = agent_with_tools(&store, "passive", "harness:claude-sdk", Vec::new());
        let spec = store.launch_spec(&agent, Some("/targets/crescendo-ai"));

        assert_eq!(spec.cwd, std::path::Path::new("/targets/crescendo-ai"));
        assert!(
            spec.env
                .iter()
                .any(|(k, v)| k == "ADI_WORKDIR" && v == "/targets/crescendo-ai"),
            "{:?}",
            spec.env
        );
    }

    /// The seam this stage moved: the spec hands the runner the agent's prompt **as written** and
    /// its tools **as data**. Nothing above the runner folds one into the other, because where tool
    /// help belongs is a fact about the engine — the same help is a system prompt for Claude and an
    /// opening user turn for Codex.
    #[test]
    fn a_launch_spec_carries_the_prompt_as_written_and_the_tools_as_data() {
        let store = scratch("spec-tools");
        let id = tool_answering_llm_help(&store, "greet", "Usage: greet <name>");
        let agent = agent_with_tools(&store, "helper", "harness:claude-sdk", vec![id]);

        let spec = store.launch_spec(&agent, None);
        assert_eq!(
            spec.system_prompt.as_deref(),
            Some("You are a careful operator."),
            "the user's prompt is untouched",
        );
        assert_eq!(spec.tools.len(), 1);
        assert_eq!(spec.tools[0].name, "greet");
        assert_eq!(
            spec.tools[0].help.as_deref(),
            Some("Usage: greet <name>"),
            "the tool's own help travels with it",
        );
        // The engine's configuration goes down whole and uninterpreted, so each runner can decode
        // its own typed arguments out of it.
        assert_eq!(
            spec.arguments["system_prompt"].as_str(),
            Some("You are a careful operator."),
        );

        // An agent with no tools asks the registry for nothing.
        let bare = agent_with_tools(&store, "bare", "harness:claude-sdk", Vec::new());
        assert!(store.launch_spec(&bare, None).tools.is_empty());
    }

    /// Open a conversation of `agent` in the session store, as a launch would. `cwd` is where it
    /// was started — the directory every later turn re-enters. Returns its id.
    fn seed_conversation(store: &Agents, agent: &str, backend: &str, cwd: &str) -> String {
        store
            .sessions()
            .create(agent, Backend::from(backend), cwd, "go")
            .expect("open a conversation")
            .id
    }

    /// Seed a live run of `agent`: a session whose runner state names *this* process, which is
    /// exactly what a running run looks like to the counter. Returns its id.
    ///
    /// Both halves of the identity, as a real spawn records them — a bare pid no longer reads as
    /// running, and should not: that is the whole point of [`live_state`].
    fn seed_live_run(store: &Agents, backend: &str, agent: &str) -> String {
        let sessions = store.sessions();
        let record = sessions
            .create(agent, Backend::from(backend), "/tmp", "seeded")
            .expect("create");
        sessions
            .session(agent, &record.id)
            .set_state(live_state())
            .expect("record a live pid");
        record.id
    }

    /// What a running child's state slot holds: this process, named by pid *and* by when it
    /// started. A test that wrote only the pid would be seeding the very ambiguity the runner now
    /// refuses to guess at.
    fn live_state() -> serde_json::Value {
        let pid = std::process::id();
        serde_json::json!({
            "pid": pid,
            "started": adi_osext::process_start_millis(pid).expect("this platform can say"),
        })
    }

    /// Live runs are counted across agents and across backends — one number, whoever started them,
    /// and whether or not the agent that started them still has a definition.
    #[test]
    fn the_run_count_spans_every_agent_and_backend() {
        let store = scratch("run-count");
        assert_eq!(store.running_count(), 0);

        seed_live_run(&store, "process:claude", "recon");
        seed_live_run(&store, "process:codex", "recon");
        seed_live_run(&store, "harness:claude-sdk", "chatty");
        assert_eq!(store.running_count(), 3);

        // A run whose child has exited doesn't count: its recorded pid names nothing alive.
        let sessions = store.sessions();
        let done = sessions
            .create("recon", Backend::ProcessClaude, "/tmp", "finished")
            .expect("create");
        sessions
            .session("recon", &done.id)
            .set_state(serde_json::json!({ "pid": 4_294_967_294u32 }))
            .expect("a pid naming nothing");
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

        seed_live_run(&store, "process:claude", "other");
        assert!(
            store.run_in("solver", "go", None).is_ok()
                || matches!(store.run_in("solver", "go", None), Err(Error::NotRunnable(_))),
            "one live run of two is below the cap, so the launch reaches the backend"
        );

        seed_live_run(&store, "harness:claude-sdk", "other");
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
        seed_live_run(&store, "harness:claude-sdk", "solver");
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

        seed_live_run(&store, "process:claude", "other");
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
        let conv = seed_conversation(&store, "chatty", "harness:claude-sdk", "/tmp");
        store
            .set_limits(RunLimits {
                max_concurrent_runs: 1,
                ..RunLimits::default()
            })
            .expect("set the limit");
        seed_live_run(&store, "process:claude", "other");

        assert_eq!(
            store.reply("chatty", &conv, "and then?").expect("reply"),
            Sent::Queued { place: 1 },
            "a full cap queues the message rather than refusing it"
        );
        // And the platform does not start it by itself while the cap is still full, even though the
        // conversation is idle and the message is waiting.
        assert_eq!(
            store.sessions().queued("chatty", &conv),
            ["and then?"],
            "the queued message is otherwise ready to start"
        );
        assert!(
            !store.advance_queue(&agent, &conv),
            "an automatic start waits for a free slot"
        );
        assert_eq!(
            store.sessions().queued("chatty", &conv),
            ["and then?"],
            "and it keeps its place rather than being dropped"
        );
    }

    /// Settling is a *reader's* act, and it must not read as the conversation having just spoken. An
    /// agent that answered overnight is committed by whoever opens the chat in the morning; stamped
    /// with that instant, every listing sorted by when a session last spoke would move the chat to
    /// the top for the sole reason that somebody looked at it.
    #[test]
    fn an_answer_settled_long_after_it_finished_keeps_the_moment_it_finished() {
        let store = scratch("settle-time");
        store
            .save("chatty", spec("harness:claude-sdk"))
            .expect("save");
        let agent = store.get("chatty").expect("get").expect("present");
        let sessions = store.sessions();
        let conv = sessions
            .create("chatty", Backend::HarnessClaudeSdk, "/tmp", "set the port")
            .expect("open a conversation")
            .id;

        let now = now_unix() * 1_000;
        let asked = now - 10_800_000; // three hours ago
        sessions
            .append_turn(
                "chatty",
                &conv,
                store::Turn {
                    at: asked,
                    ..store::user_turn("set the port")
                },
            )
            .expect("the question");
        let log = sessions.log_path("chatty", &conv);
        std::fs::write(
            &log,
            format!(
                "{}\n",
                serde_json::json!({ "type": "result", "result": "Moved it to 81." }),
            ),
        )
        .expect("write the log");
        // The engine stopped writing two hours ago, and nobody has opened the chat since.
        let finished = std::time::SystemTime::now() - Duration::from_secs(7_200);
        std::fs::File::options()
            .write(true)
            .open(&log)
            .expect("open the log")
            .set_modified(finished)
            .expect("back-date it");
        sessions
            .session("chatty", &conv)
            .set_state(serde_json::json!({ "pid": dead_pid() }))
            .expect("a finished turn");

        let turns = store.transcript(&agent, &conv);
        assert_eq!(turns.len(), 2, "{turns:?}");
        let settled = turns[1].at;
        assert!(
            settled > asked && settled < now - 3_600_000,
            "the answer is dated when the engine finished it, not when it was read: \
             {settled} is not two hours back from {now}",
        );
        assert_eq!(
            store.transcript(&agent, &conv)[1].at,
            settled,
            "and it stays there — a second reader is not a second answer",
        );
        // What the listing then sorts by is that moment; that the record reads it off the last turn
        // rather than off the files is `store::tests::only_a_turn_counts_as_activity`. Here it is
        // floored by the session's start, which this test opened a moment ago.
        let runs = store.runs(&agent);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].last_activity, runs[0].started_at.max(settled));

        let _ = std::fs::remove_dir_all(store.config.root());
    }

    /// A pid that certainly names nothing: a child run to completion and reaped. Not a made-up
    /// number — pid 1 answers "alive", which would quietly invert every assertion about a finished
    /// run. And never *this* process: stopping a session recorded against it would signal the whole
    /// test run's process group.
    fn dead_pid() -> u32 {
        #[cfg(unix)]
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn");
        #[cfg(not(unix))]
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "exit"])
            .spawn()
            .expect("spawn");
        let pid = child.id();
        let _ = child.wait();
        pid
    }

    /// The whole conversation loop, through the store and the runner rather than a backend: a turn
    /// whose child has exited is folded into the transcript on the next read, and a message said
    /// while one is still in flight waits its turn in the queue.
    #[test]
    fn a_finished_turn_settles_and_a_message_said_mid_answer_waits_its_turn() {
        let store = scratch("conv-lifecycle");
        store
            .save("chatty", spec("harness:claude-sdk"))
            .expect("save");
        let agent = store.get("chatty").expect("get").expect("present");
        let sessions = store.sessions();
        let conv = sessions
            .create("chatty", Backend::HarnessClaudeSdk, "/tmp", "set the port")
            .expect("open a conversation")
            .id;
        sessions
            .append_turn("chatty", &conv, store::user_turn("set the port"))
            .expect("the question");
        // What the turn child left in the log, and a pid that no longer names it.
        std::fs::write(
            sessions.log_path("chatty", &conv),
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "result",
                    "result": "Moved it to 81.",
                    "usage": { "input_tokens": 12, "output_tokens": 3 },
                }),
            ),
        )
        .expect("write the log");
        sessions
            .session("chatty", &conv)
            .set_state(serde_json::json!({ "pid": dead_pid() }))
            .expect("a finished turn");

        let turns = store.transcript(&agent, &conv);
        assert_eq!(turns.len(), 2, "{turns:?}");
        assert_eq!(turns[1].role, "assistant");
        assert_eq!(turns[1].text, "Moved it to 81.");
        assert!(!turns[1].pending, "its child is gone; nothing is streaming");
        assert_eq!(
            turns[1].metrics.as_ref().and_then(|m| m.input_tokens),
            Some(12),
            "the engine's telemetry comes back through the runner's events",
        );
        assert_eq!(
            sessions.turns("chatty", &conv).len(),
            2,
            "the answer was committed, not synthesized on every read",
        );
        assert_eq!(
            store.transcript(&agent, &conv).len(),
            2,
            "and reading again does not fold a second one",
        );
        // The run list speaks of the same session, with the task it was opened with.
        let runs = store.runs(&agent);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, conv);
        assert_eq!(runs[0].message, "set the port");
        assert!(!runs[0].running);

        // Now a turn *is* in flight: anything said joins the queue rather than starting a second
        // child into the same slot.
        sessions
            .session("chatty", &conv)
            .set_state(live_state())
            .expect("a live turn");
        assert_eq!(
            store.reply("chatty", &conv, "and restart it").expect("reply"),
            Sent::Queued { place: 1 },
        );
        assert_eq!(
            store.reply("chatty", &conv, "then tell me").expect("reply"),
            Sent::Queued { place: 2 },
        );
        let waiting = store.transcript(&agent, &conv);
        assert_eq!(
            waiting[2..]
                .iter()
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>(),
            ["and restart it", "then tell me"],
            "the queue trails the transcript in the order it will be asked",
        );
        assert!(waiting[2..].iter().all(|t| t.queued));
        assert!(
            store.unqueue("chatty", &conv, 0).expect("unqueue"),
            "a message can be taken back before it is ever asked",
        );
        assert_eq!(sessions.queued("chatty", &conv), ["then tell me"]);

        // Stopping drops the whole line. What was queued was written expecting the answer that is
        // being cut short, so marching on into a conversation the human just interrupted would
        // answer questions they no longer asked.
        //
        // Stopped from a *settled* pid on purpose: signalling a live one here would mean signalling
        // this test process's own group. That the signal path itself escalates TERM → KILL is
        // asserted against a real child in `runner::detached`.
        sessions
            .session("chatty", &conv)
            .set_state(serde_json::json!({ "pid": dead_pid() }))
            .expect("a finished turn");
        store.stop_run("chatty", &conv).expect("stop");
        assert!(
            sessions.queued("chatty", &conv).is_empty(),
            "stopping an answer forgets what was queued behind it",
        );

        // A backend that keeps no thread has nothing to reply into.
        store.save("solo", spec("process:claude")).expect("save");
        assert!(matches!(
            store.reply("solo", &conv, "hi"),
            Err(Error::Unsupported(_))
        ));
    }

    /// The lifecycle verbs all address one session: the run list, the snapshot, the hide flag, the
    /// stop that forgets what was queued behind it, and the delete that takes the lot.
    #[test]
    fn the_run_list_the_peek_and_the_delete_all_speak_of_the_same_session() {
        let store = scratch("run-lifecycle");
        store.save("recon", spec("process:claude")).expect("save");
        let agent = store.get("recon").expect("get").expect("present");
        let sessions = store.sessions();
        let first = sessions
            .create("recon", Backend::ProcessClaude, "/tmp", "map the site")
            .expect("create")
            .id;
        let second = sessions
            .create("recon", Backend::ProcessClaude, "/tmp", "read the bundles")
            .expect("create")
            .id;
        std::fs::write(sessions.log_path("recon", &second), "line one\nline two\n")
            .expect("write the log");

        let runs = store.runs(&agent);
        assert_eq!(runs.len(), 2, "newest first");
        assert_eq!(runs[0].run_id, second);
        assert_eq!(runs[0].message, "read the bundles");
        assert!(runs.iter().all(|r| !r.running && !r.hidden));

        let peek = store.peek_run(&agent, &second);
        assert!(!peek.interactive);
        assert!(!peek.running);
        assert_eq!(peek.output, "line one\nline two");
        assert!(peek.attach.starts_with("tail -f "), "{}", peek.attach);
        // With no run named, the newest is what a watcher means.
        assert_eq!(store.peek(&agent).output, peek.output);

        assert!(
            store.set_run_hidden("recon", &first, true).expect("hide"),
            "an existing run is there to flag",
        );
        assert!(
            store.runs(&agent).iter().any(|r| r.run_id == first && r.hidden),
            "hiding is a flag, and the run stays in the history",
        );
        assert!(
            !store.set_run_hidden("recon", "0000000000001-0000", true).expect("absent"),
            "a run that isn't there is nothing to flag",
        );

        // Stopping an answer forgets what was written expecting it.
        sessions
            .enqueue("recon", &second, "and then diff them")
            .expect("enqueue");
        assert!(
            !store.stop_run("recon", &second).expect("stop"),
            "nothing was live to signal",
        );
        assert!(sessions.queued("recon", &second).is_empty());

        assert!(store.delete_run("recon", &second).expect("delete"));
        assert_eq!(store.runs(&agent).len(), 1);
        assert!(!sessions.log_path("recon", &second).exists());
        assert!(
            !store.delete_run("recon", &second).expect("delete again"),
            "deleting what is already gone settles quietly",
        );
    }

    /// A conversation answers from the directory it started in, however the manifest resolves now —
    /// the engine's session store is keyed by cwd, so answering from elsewhere would leave it
    /// unresumable and put the files earlier turns wrote out of reach.
    #[test]
    fn a_conversation_re_enters_the_directory_it_started_in() {
        let store = scratch("conv-pinned");
        let agent = agent_with_tools(&store, "passive", "harness:claude-sdk", Vec::new());
        let conv = seed_conversation(
            &store,
            "passive",
            "harness:claude-sdk",
            "/targets/crescendo-ai",
        );
        let record = store.sessions().get("passive", &conv).expect("the session");
        assert_eq!(record.cwd, std::path::Path::new("/targets/crescendo-ai"));
        assert_eq!(
            session_dir(&store.config, &record),
            std::path::Path::new("/targets/crescendo-ai"),
        );
        // …and it is the agent's own resolve that is *not* consulted a second time.
        assert_ne!(
            workspace::resolve(&store.config, &agent.manifest, None),
            record.cwd
        );
    }

    /// Codex takes its `system_prompt` as the opening user turn, so tool help or a location block
    /// folded in would arrive as something to answer. That rule now lives in the runner, which is
    /// the layer that knows it — the spec carries the same data for every engine, and the store
    /// never edits an agent's stored prompt on the way past.
    ///
    /// What is asserted here is the store's side of it. That Codex's argv is left clean is asserted
    /// where the decision is made, in `runner::detached` and `runner::pty`.
    #[test]
    fn a_codex_agents_stored_prompt_is_never_rewritten() {
        let store = scratch("tool-help-codex");
        let id = tool_answering_llm_help(&store, "greet", "Usage: greet <name>");
        let agent = agent_with_tools(&store, "coder", "pty:codex", vec![id]);

        let spec = store.spec_in(&agent, store.config.root().to_path_buf());
        assert_eq!(
            spec.system_prompt.as_deref(),
            Some("You are a careful operator.")
        );
        assert_eq!(
            store.get("coder").expect("read back").expect("the agent"),
            agent
        );
    }

    /// A tool ticked on but no longer registered (deleted, or archived out from under the agent)
    /// contributes nothing — being the only one, it leaves the spec with no tools at all, so no
    /// runner has a section to write.
    #[test]
    fn an_unregistered_tool_adds_nothing() {
        let store = scratch("tool-help-ghost");
        let agent = agent_with_tools(
            &store,
            "haunted",
            "harness:claude-sdk",
            vec!["no-such-tool".into()],
        );
        let spec = store.spec_in(&agent, store.config.root().to_path_buf());
        assert!(spec.tools.is_empty(), "{:?}", spec.tools);
    }
}
