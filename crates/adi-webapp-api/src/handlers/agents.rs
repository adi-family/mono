use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use adi_agents::AgentManifest;
use adi_agents::Agents;
use adi_agents::Backend;
use adi_agents::Error as AgentStoreError;
use adi_agents::SecretAttachment;
use adi_agents::StoredAgent;
use adi_agents::arguments::WasmArguments;
use adi_agents::contains_json_null;

use crate::types::{
    AgentBackendOption, AgentBuildResult, AgentCapabilities, AgentCode, AgentDto, AgentFormField,
    AgentFormFieldKind, AgentFormOption, AgentFormSpec, AgentKeys, AgentPeek, AgentRef,
    AgentRunInfo, AgentRunResult, AgentRuns, AgentStep, AgentToolStatus, AgentTurn,
    AgentTurnMetrics, AgentsState, AllAgentRuns, HideRun, ProjectRunLimit, ReplyToRun, RunAgent,
    RunRef, SaveAgent, SaveAgentCode, SecretRef, SetRunLimit, UnqueueFromRun,
};

use super::files::MAX_TEXT_BYTES;
use super::response::{Response, clean, error, ok_json};

/// `GET /api/agents` — every registered agent definition. Each mutation endpoint below returns a
/// fresh [`AgentsState`], so the client refreshes from one round-trip.
#[must_use]
pub fn agents(store: &Agents) -> Response {
    match agents_state(store) {
        Ok(state) => ok_json(&state),
        Err(e) => Response::from(&e),
    }
}

/// The full [`AgentsState`]: the stored definitions decorated with live run state, plus the form
/// schema. Pty sessions are listed once; process agents consult their recorded PID. Shared with
/// the meta-handler, which reuses it to find the well-known `adi-agent` and reads back the schema.
pub(crate) fn agents_state(store: &Agents) -> Result<AgentsState, AgentStoreError> {
    let sessions = adi_agents::running_sessions();
    // Both caps and the load behind them, taken once: every row's "would this be refused?" and the
    // per-project rows below are answered from this one snapshot.
    let caps = RunCaps {
        limits: store.limits(),
        load: store.run_load(),
    };
    let project_run_limits = caps.rows();
    Ok(AgentsState {
        agents: store
            .list()?
            .into_iter()
            .map(|a| agent_dto(store, a, &sessions, &caps))
            .collect(),
        form: agent_form_spec(),
        max_concurrent_runs: caps.limits.max_concurrent_runs,
        running_runs: count(caps.load.total()),
        project_run_limits,
    })
}

/// The run caps as one page render sees them: what is allowed, and what is live.
struct RunCaps {
    limits: adi_agents::RunLimits,
    load: adi_agents::RunLoad,
}

impl RunCaps {
    /// Whether an agent filed under `project` (or none) would be refused a launch right now.
    fn blocks(&self, project: Option<&str>) -> bool {
        self.limits.is_full(self.load.total())
            || project.is_some_and(|p| self.limits.project_is_full(p, self.load.in_project(p)))
    }

    /// One row per project that has a cap of its own or something running — capped-and-idle and
    /// running-but-uncapped are both worth showing, so the page can state either.
    fn rows(&self) -> Vec<ProjectRunLimit> {
        let mut projects: std::collections::BTreeSet<&String> =
            self.limits.projects.keys().collect();
        projects.extend(self.load.projects().keys());
        projects
            .into_iter()
            .map(|project| ProjectRunLimit {
                max_concurrent_runs: self.limits.project_limit(project).unwrap_or(0),
                running_runs: count(self.load.in_project(project)),
                project: project.clone(),
            })
            .collect()
    }
}

/// A live-run tally as the wire carries it. Saturating: a machine with four billion live runs has
/// worse problems than a rounded number.
fn count(runs: usize) -> u32 {
    u32::try_from(runs).unwrap_or(u32::MAX)
}

/// `POST /api/agents/limit` — set how many runs may be live at once: the global cap, or one
/// project's own when `project` names one (`0` lifts / clears). Answers with the fresh state, so
/// the page's counters settle in the same round-trip.
#[must_use]
pub fn set_run_limit(store: &Agents, body: &[u8]) -> Response {
    let Ok(req) = serde_json::from_slice::<SetRunLimit>(body) else {
        return error(
            400,
            "expected JSON body { \"max_concurrent_runs\": <number>, \"project\"?: \"…\" } — 0 lifts the limit",
        );
    };
    let stored = match req.project.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(project) => store.set_project_limit(project, req.max_concurrent_runs),
        None => {
            let mut limits = store.limits();
            limits.max_concurrent_runs = req.max_concurrent_runs;
            store.set_limits(limits)
        }
    };
    if let Err(e) = stored {
        return Response::from(&e);
    }
    match agents_state(store) {
        Ok(state) => ok_json(&state),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/run` — launch an agent in its backend. Pty engines start an interactive
/// session you type into, so the `message` is optional there. Headless engines (`process` /
/// `harness`) get one shot: they run a single `--print` turn on `message` as the prompt and exit,
/// so a task is **required** — launching one with no message would just have it act on a placeholder
/// and do nothing, so that is rejected (400) rather than silently run.
#[must_use]
pub fn run_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_run_agent(body) else {
        return bad_agent_ref();
    };
    let name = req.name.trim();
    let message = req.message.trim();
    let agent = match get_agent(store, name) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let interactive = agent.manifest.executor() == "pty";
    if !interactive && message.is_empty() {
        return error(
            400,
            "This backend runs headless (one --print turn), so it needs an initial task — enter what it should do before running.",
        );
    }
    // A pty backend takes no task, so a blank message is its normal launch, not a missing one.
    let message = if message.is_empty() { "run" } else { message };
    let working_dir = req
        .working_dir
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());
    // `force` is the human's "run it anyway" after a refusal — the only way past the concurrency
    // limit, and never something an automatic launch sends.
    let launch = if req.force {
        store.force_run_in(name, message, working_dir)
    } else {
        store.run_in(name, message, working_dir)
    };
    let launch = match launch {
        Ok(launch) => launch,
        Err(e) => return Response::from(&e),
    };
    let (message, run_id) = match launch {
        adi_agents::Launch::Pty { session, .. } => (
            format!("Started “{name}” in session {session} — watch it in the live view."),
            String::new(),
        ),
        adi_agents::Launch::Process {
            pid, log, run_id, ..
        } => (
            format!(
                "Started “{name}” as process {pid} — output: {}",
                log.display()
            ),
            run_id,
        ),
    };
    match agents_state(store) {
        Ok(state) => ok_json(&AgentRunResult {
            message,
            run_id,
            state,
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/runs` — a headless agent's run history, newest first (each Run is an independent
/// run of the agent's settings). Interactive (pty) agents keep no history and answer `runs: []`.
#[must_use]
pub fn agent_runs(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    match get_agent(store, req.name.trim()) {
        Ok(agent) => ok_json(&runs_response(store, &agent)),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/run/peek` — a read-only snapshot of one specific run's log (or the pty screen
/// for an interactive backend). A run that has produced nothing answers with empty output, not 404.
/// For a harness backend the run is an answerable conversation, so the snapshot also carries its
/// turn-by-turn transcript (`turns`) and `answerable: true`.
#[must_use]
pub fn peek_run(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_run_ref(body) else {
        return bad_run_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let run_id = req.run_id.trim();
    let peek = store.peek_run(&agent, run_id);
    let caps = agent_caps(&agent);
    // Any backend that produces turns (conversations, or one-shot runs synthesized as one answered
    // turn) feeds the same progress view; the transcript is empty for the rest (e.g. pty).
    let turns = store
        .transcript(&agent, run_id)
        .into_iter()
        .map(agent_turn)
        .collect();
    ok_json(&AgentPeek {
        name: agent.name.clone(),
        running: peek.running,
        output: peek.output,
        attach: peek.attach,
        interactive: peek.interactive,
        run_id: run_id.to_string(),
        answerable: caps.answerable,
        caps,
        turns,
    })
}

/// `POST /api/agents/run/reply` — say something into one of a harness agent's conversations and
/// reply with a fresh snapshot (transcript included). One turn runs at a time, so the message either
/// starts the next turn or joins that conversation's queue — either way it lands in the returned
/// transcript, a queued one flagged as such. Only a backend that keeps no conversation is refused
/// (400).
#[must_use]
pub fn reply_run(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_reply_to_run(body) else {
        return bad_reply_to_run();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let run_id = req.run_id.trim();
    if let Err(e) = store.reply(&agent.name, run_id, req.message.trim()) {
        return Response::from(&e);
    }
    conversation_snapshot(store, &agent, run_id)
}

/// `POST /api/agents/run/unqueue` — drop one message from a conversation's queue before it is asked,
/// and reply with a fresh snapshot. Idempotent: an index that is no longer queued (it started its
/// turn a moment ago) simply changes nothing.
#[must_use]
pub fn unqueue_run(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_unqueue_from_run(body) else {
        return bad_unqueue_from_run();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let run_id = req.run_id.trim();
    if let Err(e) = store.unqueue(&agent.name, run_id, req.index) {
        return Response::from(&e);
    }
    conversation_snapshot(store, &agent, run_id)
}

/// A conversation's fresh snapshot — the answer to every write into it, so the sender sees their
/// message (and the answer already streaming under it) without waiting for the next poll.
fn conversation_snapshot(store: &Agents, agent: &StoredAgent, run_id: &str) -> Response {
    let peek = store.peek_run(agent, run_id);
    let turns = store
        .transcript(agent, run_id)
        .into_iter()
        .map(agent_turn)
        .collect();
    ok_json(&AgentPeek {
        name: agent.name.clone(),
        running: peek.running,
        output: peek.output,
        attach: peek.attach,
        interactive: peek.interactive,
        run_id: run_id.to_string(),
        answerable: true,
        caps: agent_caps(agent),
        turns,
    })
}

/// `POST /api/agents/run/stop` — stop one specific run, then report the fresh run history. For a
/// conversation this also drops anything queued behind the answer being cut short. Idempotent for an
/// already-finished run; only an unknown agent is a 404.
#[must_use]
pub fn stop_run(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_run_ref(body) else {
        return bad_run_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    if let Err(e) = store.stop_run(&agent.name, req.run_id.trim()) {
        return Response::from(&e);
    }
    ok_json(&runs_response(store, &agent))
}

/// `POST /api/agents/run/delete` — delete one run outright and report the fresh run history. For a
/// harness agent this is the whole conversation: transcript, log, queue and all. A live run is
/// stopped first. Idempotent for a run that is already gone; only an unknown agent is a 404, and a
/// backend that keeps no run history is a 400.
#[must_use]
pub fn delete_run(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_run_ref(body) else {
        return bad_run_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    if let Err(e) = store.delete_run(&agent.name, req.run_id.trim()) {
        return Response::from(&e);
    }
    ok_json(&runs_response(store, &agent))
}

/// `POST /api/agents/run/hide` — hide one session from the chat rail, or bring it back
/// (`hidden: false`), then report the fresh run history. Only a listing preference: the run keeps
/// running and keeps everything it has written, and the history still carries it — flagged `hidden`,
/// which is what the rail leaves out. Idempotent, and for a run that is already gone a no-op; only an
/// unknown agent is a 404, and a backend that keeps no run history is a 400.
#[must_use]
pub fn hide_run(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_hide_run(body) else {
        return bad_hide_run();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    if let Err(e) = store.set_run_hidden(&agent.name, req.run_id.trim(), req.hidden) {
        return Response::from(&e);
    }
    ok_json(&runs_response(store, &agent))
}

/// `GET /api/agents/runs/all` — the run history of every agent in one round-trip, for the
/// cross-agent chat index. One [`AgentRuns`] per agent (same shape as `/api/agents/runs`), in the
/// store's list order; the client flattens and sorts them.
#[must_use]
pub fn all_agent_runs(store: &Agents) -> Response {
    match store.list() {
        Ok(agents) => {
            let agents = agents.iter().map(|a| runs_response(store, a)).collect();
            ok_json(&AllAgentRuns { agents })
        }
        Err(e) => Response::from(&e),
    }
}

/// Build the [`AgentRuns`] history answer for an agent.
fn runs_response(store: &Agents, agent: &StoredAgent) -> AgentRuns {
    let caps = agent_caps(agent);
    AgentRuns {
        name: agent.name.clone(),
        interactive: caps.interactive,
        answerable: caps.answerable,
        caps,
        runs: store
            .runs(agent)
            .into_iter()
            .map(|r| AgentRunInfo {
                run_id: r.run_id,
                started_at: r.started_at,
                last_activity: r.last_activity,
                message: r.message,
                running: r.running,
                hidden: r.hidden,
            })
            .collect(),
    }
}

/// The backend's capability profile as a wire [`AgentCapabilities`].
fn agent_caps(agent: &StoredAgent) -> AgentCapabilities {
    let c = adi_agents::capabilities(&agent.manifest.backend);
    AgentCapabilities {
        interactive: c.interactive,
        history: c.history,
        answerable: c.answerable,
        live_text: c.live_text,
        tool_steps: c.tool_steps,
        thinking: c.thinking,
        metrics: c.metrics,
    }
}

/// Map a store [`adi_agents::Turn`] onto its wire [`AgentTurn`], including its steps and metrics.
fn agent_turn(t: adi_agents::Turn) -> AgentTurn {
    AgentTurn {
        role: t.role,
        text: t.text,
        at: t.at,
        pending: t.pending,
        queued: t.queued,
        steps: t.steps.into_iter().map(agent_step).collect(),
        metrics: t.metrics.map(agent_metrics),
    }
}

/// Map a store [`adi_agents::Step`] onto its wire [`AgentStep`].
fn agent_step(s: adi_agents::Step) -> AgentStep {
    match s {
        adi_agents::Step::Message { text } => AgentStep::Message { text },
        adi_agents::Step::Thinking { text } => AgentStep::Thinking { text },
        adi_agents::Step::Tool {
            name,
            input,
            status,
            output,
        } => AgentStep::Tool {
            name,
            input,
            status: match status {
                adi_agents::ToolStatus::Running => AgentToolStatus::Running,
                adi_agents::ToolStatus::Ok => AgentToolStatus::Ok,
                adi_agents::ToolStatus::Error => AgentToolStatus::Error,
            },
            output,
        },
    }
}

/// Map store [`adi_agents::TurnMetrics`] onto their wire [`AgentTurnMetrics`].
fn agent_metrics(m: adi_agents::TurnMetrics) -> AgentTurnMetrics {
    AgentTurnMetrics {
        input_tokens: m.input_tokens,
        output_tokens: m.output_tokens,
        cost_micro_usd: m.cost_micro_usd,
        duration_ms: m.duration_ms,
        num_turns: m.num_turns,
        permission_denials: m.permission_denials,
        is_error: m.is_error,
    }
}

/// `POST /api/agents/save` — create or update an agent definition (an upsert keyed by `name`),
/// then report the fresh list. `name` and `backend` are required. Passing `rename_from` renames an
/// existing agent to `name` before applying the edit, instead of leaving the old manifest behind.
#[must_use]
pub fn save_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_save_agent(body) else {
        return bad_save_agent();
    };
    if req.arguments.values().any(contains_json_null) {
        return error(
            400,
            "agent arguments cannot contain null (the manifest store is TOML)",
        );
    }
    let name = req.name.trim().to_string();
    // Move the manifest first, so the save below is an ordinary edit of an existing file — that is
    // what preserves `created_at`. A failed rename must abort the save, or the edit would land on
    // a fresh agent and strand the original.
    if let Some(from) = clean(req.rename_from).filter(|from| *from != name) {
        if let Err(e) = store.rename(&from, &name) {
            return Response::from(&e);
        }
    }
    // `path` and `env` are edited by the full agent form alone. Read the stored agent (post-rename,
    // so an edit that renames still finds it) and carry them over when this request left them out —
    // otherwise saving from a form that never offered them would quietly wipe an agent's toolchain.
    let stored = store.get(&name).ok().flatten().map(|a| a.manifest);
    let manifest = AgentManifest {
        backend: Backend::from(req.backend.trim()),
        arguments: clean_arguments(req.arguments),
        tags: req
            .tags
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        starred: req.starred,
        project: clean(req.project),
        // The adi tools enabled for this agent (its per-tool checkboxes) — each becomes a shim in
        // the agent's own `.bin` at launch. Trimmed and de-blanked; order + dedup left to the store.
        bin_tools: req
            .bin_tools
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        // The secrets attached to this agent (its per-secret checkboxes). Only these are decrypted
        // and injected into the agent's runs. A blank scope is normalized to `None` (global).
        secrets: req
            .secrets
            .into_iter()
            .filter_map(secret_attachment)
            .collect(),
        // Extra `PATH` dirs and env vars: what the request states, else what the agent already had.
        // Blank entries are dropped here so an empty line left in a textarea can't put an empty dir
        // on the run's `PATH` or an unnamed variable in its environment.
        path: match req.path {
            Some(dirs) => dirs
                .into_iter()
                .map(|dir| dir.trim().to_string())
                .filter(|dir| !dir.is_empty())
                .collect(),
            None => stored.as_ref().map(|m| m.path.clone()).unwrap_or_default(),
        },
        env: match req.env {
            Some(vars) => vars
                .into_iter()
                .map(|(key, value)| (key.trim().to_string(), value))
                .filter(|(key, _)| !key.is_empty())
                .collect(),
            None => stored.as_ref().map(|m| m.env.clone()).unwrap_or_default(),
        },
        // The store owns the timestamps.
        created_at: 0,
        updated_at: 0,
    };
    match store.save(&name, manifest) {
        Ok(_) => agents(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/delete` — delete an agent definition, then report the fresh list.
#[must_use]
pub fn delete_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    match store.delete(req.name.trim()) {
        Ok(_) => agents(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/peek` — a read-only snapshot of a running agent's pty screen, for the live
/// view. A registered agent without a live session answers `running: false` (200, not an error);
/// only an unknown name is a 404.
#[must_use]
pub fn peek_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    match get_agent(store, req.name.trim()) {
        Ok(agent) => peek_response(store, &agent),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/send-keys` — type into a running agent's pty session (the interactive
/// half of the live view): `text` is sent literally, then `key` is pressed. Replies with a
/// fresh screen snapshot after a short settle delay, so the sender sees the effect immediately.
#[must_use]
pub fn send_agent_keys(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_keys(body) else {
        return bad_agent_keys();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    if let Err(e) = adi_agents::send_keys(&agent.name, &req.text, &req.key) {
        return Response::from(&e);
    }
    // Give the TUI a beat to redraw, so the response snapshot already shows the keystrokes.
    std::thread::sleep(std::time::Duration::from_millis(120));
    peek_response(store, &agent)
}

/// `POST /api/agents/stop` — stop a live pty session or detached process, then report the fresh
/// list. Idempotent for an already-stopped agent; only an unknown definition is a 404.
#[must_use]
pub fn stop_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    match store.stop(&agent.name) {
        Ok(_) => agents(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/code` — read the employee source file a wasm agent's `src` argument points
/// at, for the code editor on the Agents page.
#[must_use]
pub fn agent_code(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let src = match agent_src(&agent) {
        Ok(src) => src,
        Err(resp) => return resp,
    };
    match std::fs::metadata(&src) {
        Ok(meta) if meta.len() > MAX_TEXT_BYTES => {
            return error(
                400,
                &format!(
                    "{src} is too large to edit ({} bytes, max {MAX_TEXT_BYTES})",
                    meta.len()
                ),
            );
        }
        _ => {}
    }
    match std::fs::read_to_string(&src) {
        Ok(code) => ok_json(&AgentCode {
            name: agent.name,
            path: src,
            code,
        }),
        Err(e) => error(400, &format!("couldn't read {src}: {e}")),
    }
}

/// `POST /api/agents/code/save` — write the code editor's buffer back to the wasm agent's
/// `src` file, replying with the fresh [`AgentCode`].
#[must_use]
pub fn save_agent_code(store: &Agents, body: &[u8]) -> Response {
    let Ok(req) = serde_json::from_slice::<SaveAgentCode>(body) else {
        return error(
            400,
            "expected JSON body { \"name\": \"…\", \"code\": \"…\" }",
        );
    };
    if req.name.trim().is_empty() {
        return error(
            400,
            "expected JSON body { \"name\": \"…\", \"code\": \"…\" }",
        );
    }
    if req.code.len() as u64 > MAX_TEXT_BYTES {
        return error(
            400,
            &format!("source too large to save (max {MAX_TEXT_BYTES} bytes)"),
        );
    }
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let src = match agent_src(&agent) {
        Ok(src) => src,
        Err(resp) => return resp,
    };
    match std::fs::write(&src, req.code.as_bytes()) {
        Ok(()) => ok_json(&AgentCode {
            name: agent.name,
            path: src,
            code: req.code,
        }),
        Err(e) => error(500, &format!("couldn't write {src}: {e}")),
    }
}

/// `POST /api/agents/build` — compile a wasm agent's `src` TypeScript into its component:
/// `node <src dir>/node_modules/@adi-family/workforce-sdk/build.mjs <src> -o <src dir>/build`.
/// Blocks for the build (a few seconds), replies with its combined output. A successful build
/// fills in an empty `wasm` argument with the compiled path, making the agent dispatchable.
#[must_use]
pub fn build_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let src = match agent_src(&agent) {
        Ok(src) => PathBuf::from(src),
        Err(resp) => return resp,
    };
    let Some(dir) = src.parent().map(Path::to_path_buf) else {
        return error(400, "the src argument has no parent directory");
    };
    let build_mjs = dir.join("node_modules/@adi-family/workforce-sdk/build.mjs");
    if !build_mjs.exists() {
        return error(
            400,
            &format!(
                "no workforce SDK next to the source ({} missing) — run `npm install` in {} first",
                build_mjs.display(),
                dir.display()
            ),
        );
    }
    let Some(node) = node_bin() else {
        return error(
            500,
            "no node binary found (tried $ADI_NODE, PATH, /opt/homebrew/bin, /usr/local/bin)",
        );
    };
    let out_dir = dir.join("build");

    // jco runs via a `#!/usr/bin/env node` shebang, so the child's PATH must reach node even
    // when this server was launched with a minimal LaunchAgent environment.
    let mut path_env = std::env::var("PATH").unwrap_or_default();
    if let Some(node_dir) = Path::new(&node)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
    {
        path_env = format!("{}:{path_env}", node_dir.display());
    }

    let output = std::process::Command::new(&node)
        .arg(&build_mjs)
        .arg(&src)
        .arg("-o")
        .arg(&out_dir)
        .current_dir(&dir)
        .env("PATH", path_env)
        .output();
    let out = match output {
        Ok(out) => out,
        Err(e) => return error(500, &format!("couldn't spawn {node}: {e}")),
    };

    let mut text = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(stderr.trim_end());
    }
    let ok = out.status.success();

    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let wasm = out_dir.join(format!("{stem}.wasm")).display().to_string();
    // First successful build wires the component up; an explicit `wasm` argument is respected.
    let typed_manifest = agent.manifest.clone().into_typed::<WasmArguments>();
    if ok
        && typed_manifest
            .as_ref()
            .is_ok_and(|manifest| manifest.arguments.wasm.as_deref().is_none_or(str::is_empty))
    {
        let mut manifest = match typed_manifest {
            Ok(manifest) => manifest,
            Err(error) => return Response::from(&error),
        };
        manifest.arguments.wasm = Some(wasm.clone());
        if let Err(e) = store.save(&agent.name, manifest) {
            return Response::from(&e);
        }
    }

    match agents_state(store) {
        Ok(state) => ok_json(&AgentBuildResult {
            ok,
            output: text,
            wasm,
            state,
        }),
        Err(e) => Response::from(&e),
    }
}

/// The employee source path from an agent's `src` argument, or the 400 explaining how to set it.
fn agent_src(agent: &StoredAgent) -> Result<String, Response> {
    let arguments = agent
        .manifest
        .typed_arguments::<WasmArguments>()
        .map_err(|error| Response::from(&error))?;
    arguments.src.filter(|s| !s.is_empty()).ok_or_else(|| {
        error(
            400,
            &format!(
                "agent {} has no `src` argument pointing at its TypeScript source — \
                     set the Source path in the form (or --argument src=/path/to/employee.ts)",
                agent.name
            ),
        )
    })
}

/// The node binary the build runs with: `$ADI_NODE`, then PATH, then the usual install spots.
fn node_bin() -> Option<String> {
    if let Ok(node) = std::env::var("ADI_NODE")
        && !node.is_empty()
    {
        return Some(node);
    }
    if std::process::Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return Some("node".to_string());
    }
    ["/opt/homebrew/bin/node", "/usr/local/bin/node"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .map(ToString::to_string)
}

/// Look an agent up, folding "not registered" into [`AgentStoreError::NotFound`] (→ 404).
fn get_agent(store: &Agents, name: &str) -> Result<StoredAgent, AgentStoreError> {
    store
        .get(name)?
        .ok_or_else(|| AgentStoreError::NotFound(name.to_string()))
}

/// The [`AgentPeek`] answer for an agent: a pty screen capture for interactive backends, or the tail
/// of the detached run's log for the headless backends (which persists after the run ends). A
/// registered agent with nothing to show answers `running: false` with empty output, not an error.
fn peek_response(store: &Agents, agent: &StoredAgent) -> Response {
    let peek = store.peek(agent);
    ok_json(&AgentPeek {
        name: agent.name.clone(),
        running: peek.running,
        output: peek.output,
        attach: peek.attach,
        interactive: peek.interactive,
        run_id: String::new(),
        // A name-based peek isn't scoped to a run, so it carries no transcript. The progress feed is
        // driven by the run-scoped `peek_run` / `reply_run` above.
        answerable: false,
        caps: agent_caps(agent),
        turns: Vec::new(),
    })
}

/// Flatten a stored agent into its wire [`AgentDto`], computing adapter and live run state.
fn agent_dto(
    store: &Agents,
    agent: StoredAgent,
    sessions: &std::collections::BTreeSet<String>,
    caps: &RunCaps,
) -> AgentDto {
    let executor = agent.manifest.executor().to_string();
    let runnable = adi_agents::is_runnable(&agent.manifest);
    let running = if executor == "pty" {
        sessions.contains(&adi_agents::session_name(&agent.name))
    } else {
        store.is_running(&agent)
    };
    // Whether *this* agent is the one that would be refused: the global cap binds everybody, a
    // project cap only that project's agents.
    let at_run_limit = caps.blocks(agent.manifest.project.as_deref());
    let m = agent.manifest;
    AgentDto {
        name: agent.name,
        backend: m.backend.to_string(),
        arguments: m.arguments,
        executor,
        tags: m.tags,
        starred: m.starred,
        project: m.project,
        bin_tools: m.bin_tools,
        secrets: m
            .secrets
            .into_iter()
            .map(|s| SecretRef {
                project: s.project,
                name: s.name,
            })
            .collect(),
        path: m.path,
        env: m.env,
        created_at: m.created_at,
        updated_at: m.updated_at,
        runnable,
        running,
        at_run_limit,
    }
}

/// Normalize a wire [`SecretRef`] into a store [`SecretAttachment`], trimming the name and folding
/// a blank scope to `None` (global). Dropped (→ `None`) when the name is blank after trimming.
fn secret_attachment(reference: SecretRef) -> Option<SecretAttachment> {
    let name = reference.name.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let project = reference
        .project
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());
    Some(SecretAttachment { project, name })
}

/// The agentic-loop backend that picks its model provider at definition time (the `provider`
/// argument); every other backend has its engine baked into the `executor:what` id.
const ADI_HARNESS: &str = "harness:adi";

/// The adi-workforce employee backend: a compiled WASM component (TS → jco) the bundled engine
/// dispatches messages into. The component is named by the `wasm` argument.
const WASM_LOOP: &str = "wasm:loop-script";

/// The backends whose engine is the Claude CLI/SDK, whatever the executor.
const CLAUDE_BACKENDS: &[&str] = &["pty:claude", "process:claude", "harness:claude-sdk"];

/// The backends whose engine is the Codex CLI.
const CODEX_BACKENDS: &[&str] = &["pty:codex", "process:codex"];

/// The built-in Claude Code tools offered as one-tap toggles on the allow/deny tool pickers.
/// These are the bare tool names; a scoped specifier (e.g. `Bash(git *)`) is still typed by hand
/// into the same field. Kept in the order they read best in the picker, not alphabetically.
const CLAUDE_TOOLS: &[&str] = &[
    "Read",
    "Edit",
    "Write",
    "Bash",
    "Glob",
    "Grep",
    "Task",
    "TodoWrite",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "BashOutput",
    "KillShell",
    "ExitPlanMode",
    "SlashCommand",
];

/// Suggested models per backend, offered as one-tap chips on the Model picker. These mirror each
/// backend's `model_placeholder` — the canonical aliases/ids for that engine — while the field
/// stays free text for anything else (a full id, a provider-specific or local model).
const CLAUDE_CLI_MODELS: &[&str] = &["opus", "sonnet", "haiku", "fable"];
const CLAUDE_SDK_MODELS: &[&str] = &["claude-opus-4-8", "claude-sonnet-5", "claude-haiku-4-5"];
const CODEX_MODELS: &[&str] = &["gpt-5-codex"];
const ADI_MODELS: &[&str] = &["kimi-k3", "kimi-k2.6", "gemini-2.5-pro"];

/// Static backend/form metadata for the Agents page. This lives server-side so the API defines
/// both the selectable backends and the field shape the client renders. Backends are
/// `executor:what` pairs — the executor (`pty` / `process` / `harness` / `wasm`) is the run
/// mechanism, the suffix is what it runs.
#[allow(clippy::too_many_lines)]
fn agent_form_spec() -> AgentFormSpec {
    let mut fields = Vec::new();

    let mut name = agent_field("name", "Name", AgentFormFieldKind::Text);
    name.placeholder = "athz-solver".into();
    name.hint = "a task tagged this name auto-starts it".into();
    name.mono = true;
    name.required = true;
    fields.push(name);

    let mut backend = agent_field("backend", "Backend", AgentFormFieldKind::Select);
    backend.required = true;
    fields.push(backend);

    // The project the agent is filed under (or global). The options are the registered
    // projects, which only the client knows live — it special-cases this field by name and
    // fills the select from its projects state, like the Triggers form does.
    let mut project = agent_field("project", "Project", AgentFormFieldKind::Select);
    project.hint = "shows on that project's page".into();
    fields.push(project);

    // The adi harness runs its own agentic loop and needs to know which provider API to call;
    // provider-specific knobs below are scoped to this choice via `providers`.
    let mut provider = field_ids(
        "provider",
        "Provider",
        AgentFormFieldKind::Select,
        &[ADI_HARNESS],
    );
    provider.options = opts(&[
        ("", "— pick a provider —"),
        ("anthropic", "Anthropic"),
        ("openai", "OpenAI"),
        ("gemini", "Gemini"),
        ("monshoot", "Monshoot"),
        ("ollama", "Ollama (local)"),
    ]);
    provider.hint = "model provider the adi loop calls".into();
    fields.push(provider);

    let mut model = agent_field("model", "Model", AgentFormFieldKind::ModelPicker);
    model.placeholder = "model alias".into();
    model.hint = "tap a suggestion for the chosen backend, or type any model".into();
    model.mono = true;
    model.wide = true;
    fields.push(model);

    // ---- wasm employees (adi-workforce) ----
    let mut src = txt_field(
        "src",
        "Source path",
        &[WASM_LOOP],
        "/path/to/employee.ts",
        "TypeScript source the Code editor edits and builds",
    );
    src.wide = true;
    fields.push(src);

    let mut wasm = txt_field(
        "wasm",
        "Component path",
        &[WASM_LOOP],
        "/path/to/agent.wasm",
        "compiled component; a successful Build fills this in",
    );
    wasm.wide = true;
    fields.push(wasm);

    // ---- claude engines (any executor) ----
    let mut permission = field_ids(
        "permission_mode",
        "Permission mode",
        AgentFormFieldKind::Select,
        CLAUDE_BACKENDS,
    );
    permission.options = opts(&[
        ("", "— default —"),
        ("acceptEdits", "acceptEdits"),
        ("auto", "auto"),
        ("bypassPermissions", "bypassPermissions"),
        ("manual", "manual"),
        ("dontAsk", "dontAsk"),
        ("plan", "plan"),
    ]);
    fields.push(permission);

    fields.push(for_providers(
        sel_field(
            "effort",
            "Effort",
            CLAUDE_BACKENDS,
            opts(&[
                ("", "— default —"),
                ("low", "low"),
                ("medium", "medium"),
                ("high", "high"),
                ("xhigh", "xhigh"),
                ("max", "max"),
            ]),
            "thinking / reasoning depth",
        ),
        &["anthropic"],
    ));

    fields.push(sel_field(
        "output_format",
        "Output format",
        &["process:claude"],
        opts(&[
            ("", "text (default)"),
            ("json", "json"),
            ("stream-json", "stream-json"),
        ]),
        "how the run result is emitted",
    ));

    fields.push(tools_field(
        "allowed_tools",
        "Allowed tools",
        "Bash(git *) Edit Read",
        "built-in tools to allow — tap to toggle, or type a scoped rule like Bash(git *)",
    ));

    fields.push(tools_field(
        "disallowed_tools",
        "Disallowed tools",
        "Bash(rm *) WebFetch",
        "built-in tools to deny — tap to toggle, or type a scoped rule like Bash(rm *)",
    ));

    fields.push(num_field(
        "max_budget_usd",
        "Max budget (USD)",
        &["process:claude"],
        "e.g. 5",
        "hard spend cap (print mode)",
    ));

    fields.push(txt_field(
        "fallback_model",
        "Fallback model",
        &["process:claude", "harness:claude-sdk"],
        "sonnet",
        "used when the primary model is overloaded",
    ));

    let mut append = field_ids(
        "append_system_prompt",
        "Append system prompt",
        AgentFormFieldKind::Textarea,
        CLAUDE_BACKENDS,
    );
    append.placeholder = "Appended after the default system prompt…".into();
    append.wide = true;
    fields.push(append);

    // ---- codex engines (any executor) ----
    fields.push(sel_field(
        "sandbox",
        "Sandbox",
        CODEX_BACKENDS,
        opts(&[
            ("", "— default —"),
            ("read-only", "read-only"),
            ("workspace-write", "workspace-write"),
            ("danger-full-access", "danger-full-access"),
        ]),
        "filesystem / exec sandbox policy",
    ));

    fields.push(sel_field(
        "approval",
        "Approval",
        CODEX_BACKENDS,
        opts(&[
            ("", "— default —"),
            ("untrusted", "untrusted"),
            ("on-request", "on-request"),
            ("never", "never"),
        ]),
        "when to ask before running a command",
    ));

    fields.push(for_providers(
        sel_field(
            "reasoning_effort",
            "Reasoning effort",
            CODEX_BACKENDS,
            opts(&[
                ("", "— default —"),
                ("low", "low"),
                ("medium", "medium"),
                ("high", "high"),
            ]),
            "reasoning depth",
        ),
        &["openai"],
    ));

    // Codex takes it as its own `-C` flag; the harness instead starts the run's process there.
    // Either way it is the same question — which directory is this agent's home — so it is one
    // field. Unset falls through to the agent's project directory, then to the store root.
    fields.push(txt_field(
        "working_dir",
        "Working dir",
        &["pty:codex", "process:codex", "harness:claude-sdk"],
        "/path/to/repo",
        "where the agent starts (default: its project's directory, else the store root)",
    ));

    fields.push(chk_field(
        "skip_git_repo_check",
        "Skip git-repo check",
        &["process:codex"],
    ));
    fields.push(chk_field("web_search", "Web search", CODEX_BACKENDS));
    fields.push(chk_field("json_events", "JSONL events", &["process:codex"]));

    // ---- pty/process shared (a vendor CLI runs either way) ----
    let mut add_dir = field_executors(
        "add_dir",
        "Add dir",
        AgentFormFieldKind::Text,
        &["pty", "process"],
    );
    add_dir.placeholder = "/extra/writable/dir".into();
    add_dir.hint = "additional writable directory".into();
    add_dir.mono = true;
    add_dir.wide = true;
    fields.push(add_dir);

    // ---- harness:adi provider knobs (scoped to the `provider` argument) ----
    fields.push(for_providers(
        sel_field(
            "thinking",
            "Thinking",
            &[],
            opts(&[
                ("", "— default —"),
                ("adaptive", "adaptive"),
                ("disabled", "disabled"),
            ]),
            "extended-thinking mode",
        ),
        &["anthropic"],
    ));

    fields.push(for_providers(
        num_field(
            "frequency_penalty",
            "Frequency penalty",
            &[],
            "-2.0 – 2.0",
            "",
        ),
        &["openai"],
    ));
    fields.push(for_providers(
        num_field(
            "presence_penalty",
            "Presence penalty",
            &[],
            "-2.0 – 2.0",
            "",
        ),
        &["openai", "monshoot"],
    ));
    fields.push(for_providers(
        sel_field(
            "response_format",
            "Response format",
            &[],
            opts(&[
                ("", "— default —"),
                ("text", "text"),
                ("json_object", "json_object"),
                ("json_schema", "json_schema"),
            ]),
            "structured output",
        ),
        &["openai", "monshoot"],
    ));

    fields.push(for_providers(
        num_field(
            "thinking_budget",
            "Thinking budget",
            &[],
            "tokens",
            "thinkingConfig budget",
        ),
        &["gemini"],
    ));

    fields.push(for_providers(
        num_field(
            "num_ctx",
            "Context size",
            &[],
            "e.g. 8192",
            "context window (num_ctx)",
        ),
        &["ollama"],
    ));
    fields.push(for_providers(
        num_field("repeat_penalty", "Repeat penalty", &[], "e.g. 1.1", ""),
        &["ollama"],
    ));
    fields.push(for_providers(
        num_field("min_p", "Min-p", &[], "0.0 – 1.0", ""),
        &["ollama"],
    ));
    fields.push(for_providers(
        txt_field(
            "keep_alive",
            "Keep alive",
            &[],
            "5m / -1",
            "how long to keep the model loaded",
        ),
        &["ollama"],
    ));
    fields.push(for_providers(
        chk_field("think", "Thinking", &[]),
        &["ollama"],
    ));
    fields.push(for_providers(
        sel_field(
            "format",
            "Response format",
            &[],
            opts(&[("", "— default —"), ("json", "json")]),
            "structured output",
        ),
        &["ollama"],
    ));

    // ---- harness:adi sampling (provider-scoped) ----
    // temperature is left OFF the providers where a non-default value 400s: Anthropic current
    // models, OpenAI o-series/gpt-5, and Monshoot kimi-k2.6 (verified). It stays only where it's
    // a normal knob — Gemini and Ollama.
    fields.push(for_providers(
        num_field("temperature", "Temperature", &[], "0.0 – 2.0", ""),
        &["gemini", "ollama"],
    ));
    fields.push(for_providers(
        num_field("top_p", "Top-p", &[], "0.0 – 1.0", ""),
        &["openai", "gemini", "monshoot", "ollama"],
    ));
    fields.push(for_providers(
        num_field("top_k", "Top-k", &[], "e.g. 40", ""),
        &["gemini", "ollama"],
    ));
    fields.push(for_providers(
        num_field("seed", "Seed", &[], "e.g. 42", "deterministic sampling"),
        &["openai", "gemini", "ollama"],
    ));

    // ---- harness:adi shared (whatever the provider) ----
    let mut max_tokens = field_ids(
        "max_tokens",
        "Max output tokens",
        AgentFormFieldKind::Number,
        &[ADI_HARNESS],
    );
    max_tokens.placeholder = "e.g. 4096".into();
    max_tokens.hint = "maps to each provider's output-cap field".into();
    max_tokens.numeric = true;
    fields.push(max_tokens);

    let mut stop = field_ids(
        "stop",
        "Stop sequences",
        AgentFormFieldKind::Text,
        &[ADI_HARNESS],
    );
    stop.placeholder = "comma-separated".into();
    stop.hint = "stop generation on these strings".into();
    stop.mono = true;
    stop.wide = true;
    fields.push(stop);

    let mut max_turns = field_ids(
        "max_turns",
        "Max turns",
        AgentFormFieldKind::Number,
        &[ADI_HARNESS, "harness:claude-sdk", WASM_LOOP],
    );
    max_turns.placeholder = "optional".into();
    max_turns.hint = "harness cap on agent turns per run".into();
    max_turns.numeric = true;
    fields.push(max_turns);

    let mut api_key_env = field_ids(
        "api_key_env",
        "API key env",
        AgentFormFieldKind::Text,
        &[ADI_HARNESS],
    );
    api_key_env.placeholder = "OPENAI_API_KEY".into();
    api_key_env.hint = "environment variable read for the chosen provider".into();
    api_key_env.mono = true;
    fields.push(api_key_env);

    let mut base_url = field_ids(
        "base_url",
        "Base URL",
        AgentFormFieldKind::Text,
        &[ADI_HARNESS],
    );
    base_url.placeholder = "provider endpoint override".into();
    base_url.hint = "e.g. https://api.moonshot.ai/v1 · http://localhost:11434".into();
    base_url.mono = true;
    base_url.wide = true;
    fields.push(base_url);

    // ---- always shown ----
    fields.push(agent_field(
        "starred",
        "Starred",
        AgentFormFieldKind::Checkbox,
    ));

    let mut tags = agent_field("tags", "Tags", AgentFormFieldKind::Text);
    tags.placeholder = "comma-separated (dispatch / filtering)".into();
    tags.wide = true;
    fields.push(tags);

    let mut tools = field_ids(
        "tools",
        "CLI commands",
        AgentFormFieldKind::Text,
        &[ADI_HARNESS, "harness:claude-sdk", WASM_LOOP],
    );
    tools.placeholder = "tasks,projects,agents".into();
    tools.hint = "which adi-mono command groups this agent may use".into();
    tools.mono = true;
    tools.wide = true;
    fields.push(tools);

    let mut prompt = agent_field(
        "system_prompt",
        "System prompt",
        AgentFormFieldKind::Textarea,
    );
    prompt.placeholder = "The system prompt that seeds this agent...".into();
    prompt.wide = true;
    fields.push(prompt);

    AgentFormSpec {
        backends: vec![
            agent_backend(
                "pty:claude",
                "pty · Claude CLI",
                "pty",
                "opus / sonnet / fable / haiku",
                CLAUDE_CLI_MODELS,
            ),
            agent_backend(
                "pty:codex",
                "pty · Codex CLI",
                "pty",
                "gpt-5-codex",
                CODEX_MODELS,
            ),
            agent_backend(
                "process:claude",
                "process · Claude CLI",
                "process",
                "opus / sonnet / fable / haiku",
                CLAUDE_CLI_MODELS,
            ),
            agent_backend(
                "process:codex",
                "process · Codex CLI",
                "process",
                "gpt-5-codex",
                CODEX_MODELS,
            ),
            agent_backend(
                "harness:claude-sdk",
                "harness · Claude SDK",
                "harness",
                "claude-opus-4-8 / claude-sonnet-5",
                CLAUDE_SDK_MODELS,
            ),
            agent_backend(
                ADI_HARNESS,
                "harness · ADI loop",
                "harness",
                "provider model, e.g. kimi-k2.6 / gemini-2.5-pro",
                ADI_MODELS,
            ),
            agent_backend(
                WASM_LOOP,
                "wasm · Workforce employee",
                "wasm",
                "set by the employee's loop config",
                &[],
            ),
        ],
        fields,
    }
}

fn agent_backend(
    id: &str,
    label: &str,
    executor: &str,
    model_placeholder: &str,
    model_suggestions: &[&str],
) -> AgentBackendOption {
    AgentBackendOption {
        id: id.into(),
        label: label.into(),
        executor: executor.into(),
        model_placeholder: model_placeholder.into(),
        model_suggestions: strings(model_suggestions),
    }
}

fn agent_field(name: &str, label: &str, kind: AgentFormFieldKind) -> AgentFormField {
    AgentFormField {
        name: name.into(),
        label: label.into(),
        kind,
        placeholder: String::new(),
        hint: String::new(),
        options: Vec::new(),
        backend_ids: Vec::new(),
        executors: Vec::new(),
        providers: Vec::new(),
        mono: false,
        wide: false,
        numeric: false,
        required: false,
    }
}

fn agent_option(value: &str, label: &str) -> AgentFormOption {
    AgentFormOption {
        value: value.into(),
        label: label.into(),
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

/// A field visible only for specific backend ids (e.g. `pty:claude`).
fn field_ids(name: &str, label: &str, kind: AgentFormFieldKind, ids: &[&str]) -> AgentFormField {
    let mut f = agent_field(name, label, kind);
    f.backend_ids = strings(ids);
    f
}

/// A field visible for whole executors (`pty` / `process` / `harness`).
fn field_executors(
    name: &str,
    label: &str,
    kind: AgentFormFieldKind,
    executors: &[&str],
) -> AgentFormField {
    let mut f = agent_field(name, label, kind);
    f.executors = strings(executors);
    f
}

/// Also show a field when `harness:adi` targets one of these providers (on top of whatever
/// backend-id scoping the field already carries).
fn for_providers(mut f: AgentFormField, providers: &[&str]) -> AgentFormField {
    f.providers = strings(providers);
    f
}

/// A select field scoped to backend ids, with a hint.
fn sel_field(
    name: &str,
    label: &str,
    ids: &[&str],
    options: Vec<AgentFormOption>,
    hint: &str,
) -> AgentFormField {
    let mut f = field_ids(name, label, AgentFormFieldKind::Select, ids);
    f.options = options;
    f.hint = hint.into();
    f
}

/// A numeric field scoped to backend ids.
fn num_field(
    name: &str,
    label: &str,
    ids: &[&str],
    placeholder: &str,
    hint: &str,
) -> AgentFormField {
    let mut f = field_ids(name, label, AgentFormFieldKind::Number, ids);
    f.placeholder = placeholder.into();
    f.hint = hint.into();
    f.numeric = true;
    f
}

/// A monospace text field scoped to backend ids.
fn txt_field(
    name: &str,
    label: &str,
    ids: &[&str],
    placeholder: &str,
    hint: &str,
) -> AgentFormField {
    let mut f = field_ids(name, label, AgentFormFieldKind::Text, ids);
    f.placeholder = placeholder.into();
    f.hint = hint.into();
    f.mono = true;
    f
}

/// A checkbox scoped to backend ids (stored as a boolean backend argument).
fn chk_field(name: &str, label: &str, ids: &[&str]) -> AgentFormField {
    field_ids(name, label, AgentFormFieldKind::Checkbox, ids)
}

/// A tool-picker for the Claude backends: toggle chips for [`CLAUDE_TOOLS`] over a free-text
/// input, both editing the one space-separated tool spec (`--allowed-tools` / `--disallowed-tools`).
fn tools_field(name: &str, label: &str, placeholder: &str, hint: &str) -> AgentFormField {
    let mut f = field_ids(name, label, AgentFormFieldKind::ToolPicker, CLAUDE_BACKENDS);
    f.options = CLAUDE_TOOLS.iter().map(|&t| agent_option(t, t)).collect();
    f.placeholder = placeholder.into();
    f.hint = hint.into();
    f.mono = true;
    f.wide = true;
    f
}

/// Build a select-option list from `(value, label)` pairs.
fn opts(pairs: &[(&str, &str)]) -> Vec<AgentFormOption> {
    pairs.iter().map(|&(v, l)| agent_option(v, l)).collect()
}

// Map an agent-store error to an HTTP status: bad name / unrunnable backend / bad key → 400,
// missing → 404, wrong run state (already / not running) → 409, run cap full → 429, else 500.
impl From<&AgentStoreError> for Response {
    fn from(e: &AgentStoreError) -> Self {
        let status = match e {
            AgentStoreError::TooManyRunning { .. } => 429,
            AgentStoreError::Arguments(_)
            | AgentStoreError::InvalidName(_)
            | AgentStoreError::NotRunnable(_)
            | AgentStoreError::Unsupported(_)
            | AgentStoreError::InvalidKey(_) => 400,
            AgentStoreError::NotFound(_) => 404,
            AgentStoreError::Exists(_)
            | AgentStoreError::AlreadyRunning(_)
            | AgentStoreError::Busy(_)
            | AgentStoreError::NotRunning(_) => 409,
            AgentStoreError::Config(_)
            | AgentStoreError::Io(_)
            | AgentStoreError::Launch(_)
            | AgentStoreError::Session(_)
            | AgentStoreError::Process(_) => 500,
        };
        error(status, &e.to_string())
    }
}

fn parse_save_agent(body: &[u8]) -> Option<SaveAgent> {
    let req: SaveAgent = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && !req.backend.trim().is_empty()).then_some(req)
}

fn bad_save_agent() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"backend\": \"…\", … } with a non-empty name and backend",
    )
}

fn parse_agent_ref(body: &[u8]) -> Option<AgentRef> {
    let req: AgentRef = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty()).then_some(req)
}

fn parse_run_agent(body: &[u8]) -> Option<RunAgent> {
    let req: RunAgent = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty()).then_some(req)
}

fn parse_run_ref(body: &[u8]) -> Option<RunRef> {
    let req: RunRef = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && !req.run_id.trim().is_empty()).then_some(req)
}

fn bad_run_ref() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"run_id\": \"…\" } with a non-empty name and run_id",
    )
}

fn parse_hide_run(body: &[u8]) -> Option<HideRun> {
    let req: HideRun = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && !req.run_id.trim().is_empty()).then_some(req)
}

fn bad_hide_run() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"run_id\": \"…\", \"hidden\": true } with a non-empty name and run_id",
    )
}

fn parse_reply_to_run(body: &[u8]) -> Option<ReplyToRun> {
    let req: ReplyToRun = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && !req.run_id.trim().is_empty() && !req.message.trim().is_empty())
        .then_some(req)
}

fn bad_reply_to_run() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"run_id\": \"…\", \"message\": \"…\" } with all three non-empty",
    )
}

fn parse_unqueue_from_run(body: &[u8]) -> Option<UnqueueFromRun> {
    let req: UnqueueFromRun = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && !req.run_id.trim().is_empty()).then_some(req)
}

fn bad_unqueue_from_run() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"run_id\": \"…\", \"index\": 0 } with a non-empty name and run_id",
    )
}

fn bad_agent_ref() -> Response {
    error(400, "expected JSON body { \"name\": \"…\" }")
}

fn parse_agent_keys(body: &[u8]) -> Option<AgentKeys> {
    let req: AgentKeys = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && (!req.text.is_empty() || !req.key.is_empty())).then_some(req)
}

fn bad_agent_keys() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"text\": \"…\", \"key\": \"…\" } with a non-empty name and at least one of text/key",
    )
}

/// Normalize only the key at the shared top-level boundary. Argument values and nested manifests
/// are preserved exactly because their shape belongs to the selected backend.
fn clean_arguments(
    arguments: BTreeMap<String, serde_json::Value>,
) -> BTreeMap<String, serde_json::Value> {
    arguments
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.trim().to_string();
            if key.is_empty() {
                return None;
            }
            Some((key, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_config::Config;

    fn scratch(tag: &str) -> Agents {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-api-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Agents::with_config(Config::with_root(root))
    }

    /// A save body as the forms send it, with `path`/`env` left for the caller to state.
    fn body(path: Option<&str>, env: Option<&str>) -> Vec<u8> {
        let mut save = serde_json::json!({ "name": "solver", "backend": "pty:claude" });
        if let Some(dirs) = path {
            save["path"] = serde_json::json!([dirs]);
        }
        if let Some(vars) = env {
            save["env"] = serde_json::json!({ "NODE_ENV": vars });
        }
        save.to_string().into_bytes()
    }

    fn saved(store: &Agents) -> AgentManifest<adi_agents::RawAgentArguments> {
        store.get("solver").expect("get").expect("agent").manifest
    }

    /// The run environment is edited by the full agent form alone. Every *other* form — the meta
    /// setup, the project panel — posts a body without these fields, and must not wipe them.
    #[test]
    fn a_save_that_omits_the_run_environment_keeps_it() {
        let store = scratch("keep");
        assert_eq!(
            save_agent(&store, &body(Some("~/node22/bin"), Some("dev"))).status,
            200
        );

        assert_eq!(save_agent(&store, &body(None, None)).status, 200);

        let m = saved(&store);
        assert_eq!(m.path, vec!["~/node22/bin".to_string()]);
        assert_eq!(m.env.get("NODE_ENV").map(String::as_str), Some("dev"));
    }

    /// Stating them empty is how they are actually cleared — the difference an `Option` on the
    /// wire buys over a plain list.
    #[test]
    fn stating_them_empty_clears_them() {
        let store = scratch("clear");
        assert_eq!(
            save_agent(&store, &body(Some("~/node22/bin"), Some("dev"))).status,
            200
        );

        let cleared = serde_json::json!({
            "name": "solver", "backend": "pty:claude", "path": [], "env": {},
        });
        assert_eq!(
            save_agent(&store, cleared.to_string().as_bytes()).status,
            200
        );

        let m = saved(&store);
        assert!(m.path.is_empty(), "{:?}", m.path);
        assert!(m.env.is_empty(), "{:?}", m.env);
    }

    /// A blank line left in the form's textarea must not reach the run as an empty `PATH` entry —
    /// on unix an empty dir in `PATH` means the *current* directory, which is not what was asked
    /// for and is worth keeping out of an agent's runs.
    #[test]
    fn blank_entries_are_dropped_before_they_are_stored() {
        let store = scratch("blank");
        let body = serde_json::json!({
            "name": "solver",
            "backend": "pty:claude",
            "path": ["  ", "~/node22/bin", ""],
            "env": { "  ": "ignored", "NODE_ENV": "dev" },
        });
        assert_eq!(save_agent(&store, body.to_string().as_bytes()).status, 200);

        let m = saved(&store);
        assert_eq!(m.path, vec!["~/node22/bin".to_string()]);
        assert_eq!(m.env.keys().collect::<Vec<_>>(), vec!["NODE_ENV"]);
    }
}
