use std::collections::BTreeMap;

use adi_agents::AgentManifest;
use adi_agents::Agents;
use adi_agents::Backend;
use adi_agents::Error as AgentStoreError;
use adi_agents::SecretAttachment;
use adi_agents::StoredAgent;
use adi_agents::contains_json_null;

use crate::types::{
    AgentAsk, AgentBackendOption, AgentCapabilities, AgentChoice, AgentDto, AgentFormField,
    AgentFormFieldKind, AgentFormOption, AgentFormSpec, AgentKeys, AgentNearDup, AgentPeek,
    AgentQuestion, AgentRef, AgentRepeat, AgentRepeatShape, AgentReviewStarted, AgentRunInfo,
    AgentRunOutcome, AgentRunResult, AgentRuns, AgentSetupPreset, AgentSetupSecret, AgentStep, AgentTokenSite,
    AgentTokenSource, AgentTokenSplit, AgentTokens, AgentToolStatus, AgentTurn, AgentTurnMetrics,
    AgentSimBlock, AgentSimField, AgentSimFieldKind, AgentSimResult, AgentSimSection,
    AgentSimState, AgentSimTool, AgentSimTurn, AgentToken, AgentsState, AgentGoal, AgentGoals,
    AllAgentRuns, AnswerRun, CloseGoal, GoalsOf, HideRun, PendingAsk, PendingAsks, ProjectRunLimit,
    ReplyToRun, ReviewRun, RunAgent, RunRef, SaveAgent, SecretRef, SetGoal, SetRunLimit,
    SimulateAgent, SimulateTurn, UnqueueFromRun,
};

use super::response::{Response, clean, error, ok_json, parse_body};

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
        pending_question: store
            .pending_question(&agent.name, run_id)
            .as_ref()
            .map(agent_ask),
        turns,
    })
}

/// `POST /api/agents/run/tokens` — what one conversation spent its context on, and what it spent
/// twice. Takes a [`RunRef`], answers an [`AgentTokens`].
///
/// Deliberately **not** folded into [`peek_run`]: this re-tokenizes the whole transcript, which costs
/// a hundred-odd milliseconds on a long conversation, and the peek runs once a second. A reader asks
/// for this when they open the panel; nothing asks for it on their behalf.
///
/// A run with no transcript is not an error — an empty itemization is the honest answer for a
/// conversation that has not said anything yet, and 404 here would make the panel look broken on a
/// chat that is merely new.
#[must_use]
pub fn run_tokens(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_run_ref(body) else {
        return bad_run_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let run_id = req.run_id.trim();
    let turns = store.transcript(&agent, run_id);
    let report = adi_agents::analytics::analyze(&turns, adi_agents::analytics::Options::default());
    ok_json(&AgentTokens {
        name: agent.name.clone(),
        run_id: run_id.to_string(),
        encoding: report.encoding,
        total: report.total,
        by_source: report
            .by_source
            .into_iter()
            .map(|(source, tokens)| AgentTokenSplit {
                source: token_source(source),
                tokens,
            })
            .collect(),
        truncated: report.truncated,
        wasted: report.wasted,
        repeats: report
            .repeats
            .into_iter()
            .map(|r| AgentRepeat {
                preview: r.preview,
                tokens: r.tokens,
                count: r.count,
                wasted: r.wasted,
                shape: repeat_shape(r.shape),
                hint: r.shape.hint().unwrap_or_default().to_string(),
                sites: r.sites.into_iter().map(token_site).collect(),
            })
            .collect(),
        near_duplicates: report
            .near_duplicates
            .into_iter()
            .map(|g| AgentNearDup {
                preview: g.preview,
                count: g.count,
                tokens: g.tokens,
                wasted: g.wasted,
                sites: g.sites.into_iter().map(token_site).collect(),
            })
            .collect(),
    })
}

/// Map an analytics [`adi_agents::analytics::Site`] onto its wire [`AgentTokenSite`].
fn token_site(s: adi_agents::analytics::Site) -> AgentTokenSite {
    AgentTokenSite {
        turn: s.turn,
        step: s.step,
        source: token_source(s.source),
        tool: s.tool,
    }
}

/// Map an analytics [`adi_agents::analytics::Source`] onto its wire [`AgentTokenSource`].
fn token_source(s: adi_agents::analytics::Source) -> AgentTokenSource {
    use adi_agents::analytics::Source;
    match s {
        Source::User => AgentTokenSource::User,
        Source::Agent => AgentTokenSource::Agent,
        Source::Thinking => AgentTokenSource::Thinking,
        Source::ToolInput => AgentTokenSource::ToolInput,
        Source::ToolOutput => AgentTokenSource::ToolOutput,
    }
}

/// Map an analytics [`adi_agents::analytics::Shape`] onto its wire [`AgentRepeatShape`].
fn repeat_shape(s: adi_agents::analytics::Shape) -> AgentRepeatShape {
    use adi_agents::analytics::Shape;
    match s {
        Shape::Path => AgentRepeatShape::Path,
        Shape::Url => AgentRepeatShape::Url,
        Shape::Literal => AgentRepeatShape::Literal,
        Shape::Block => AgentRepeatShape::Block,
        Shape::Phrase => AgentRepeatShape::Phrase,
    }
}

/// The environment's root agent — the "primary agent" onboarding sets up, and the default reviewer.
/// Named here rather than taken from the client so a review always goes to the agent that owns this
/// machine, not to whatever the caller last had open.
const ROOT_AGENT: &str = "adi-agent";

/// `POST /api/agents/run/review` — hand one conversation to an agent and ask how it should have gone.
/// Takes a [`ReviewRun`], answers an [`AgentReviewStarted`].
///
/// Two steps, in this order for a reason. The dossier is written first (`Agents::review`), then the
/// reviewer is launched on a brief that points at it — so a launch refused by the concurrency cap
/// leaves the evidence on disk to be opened by hand, rather than throwing the analysis away with the
/// run that would have read it.
///
/// The reviewer starts in the **reviewed conversation's** directory. A workflow is a workflow
/// somewhere: the recommendations worth having are the ones that can check whether the tool being
/// proposed already exists in that tree. It is asked to propose and not to apply — see the brief in
/// [`adi_agents::review`] — so the directory is somewhere to read, not somewhere to edit.
#[must_use]
pub fn review_run(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_body::<ReviewRun>(body)
        .filter(|r| !r.name.trim().is_empty() && !r.run_id.trim().is_empty())
    else {
        return bad_run_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let reviewer_name = match req.reviewer.trim() {
        "" => ROOT_AGENT,
        named => named,
    };
    // Checked before anything is written: "no such agent" is a much better answer than a dossier
    // nobody was ever launched to read.
    let reviewer = match get_agent(store, reviewer_name) {
        Ok(reviewer) => reviewer,
        Err(_) => {
            return error(
                404,
                &format!(
                    "No agent named “{reviewer_name}” to review with. Set up your primary agent \
                     first, or name a reviewer that exists."
                ),
            );
        }
    };

    // The conversation's own directory, looked up before anything else because its absence is also
    // the answer to "does this session exist" — and the store's own 404 for that says "no such
    // agent", which is exactly the wrong thing to tell someone whose agent is fine.
    let run_id = req.run_id.trim();
    let Some(cwd) = store.run_cwd(&agent, run_id) else {
        return error(
            404,
            &format!("No conversation “{run_id}” on agent “{}”.", agent.name),
        );
    };
    // A session outlives the tree it ran in, and starting the reviewer in a directory that has since
    // been deleted would fail the launch over something the review does not depend on.
    let working_dir = cwd.is_dir().then(|| cwd.display().to_string());

    let review = match store.review(&agent, run_id, adi_agents::review::Options::default()) {
        Ok(review) => review,
        Err(e) => return Response::from(&e),
    };

    let launch = match store.run_in(&reviewer.name, &review.brief, working_dir.as_deref()) {
        Ok(launch) => launch,
        Err(e) => return Response::from(&e),
    };
    let review_run_id = match launch {
        adi_agents::Launch::Process { run_id, .. } => run_id,
        adi_agents::Launch::Pty { .. } => String::new(),
    };

    ok_json(&AgentReviewStarted {
        reviewer: reviewer.name,
        run_id: review_run_id,
        dossier: review.path.display().to_string(),
        reviewed: RunRef {
            name: agent.name,
            run_id: run_id.to_string(),
        },
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
        pending_question: store
            .pending_question(&agent.name, run_id)
            .as_ref()
            .map(agent_ask),
        turns,
    })
}

/// `POST /api/agents/run/answer` — settle the question a conversation is waiting on, and reply with
/// a fresh snapshot so the card disappears and the answer's turn appears in one round-trip.
///
/// A 404 here is the useful answer, not a failure: it means somebody else answered first, or the
/// deadline took the run's own default while this card sat open.
#[must_use]
pub fn answer_run(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_body::<AnswerRun>(body) else {
        return error(
            400,
            "expected JSON body { \"name\": \"…\", \"run_id\": \"…\", \"ask\"?: \"…\", \"replies\": [\"…\"] }",
        );
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let run_id = req.run_id.trim();
    let ask = clean(req.ask);
    if let Err(e) = store.answer(&agent.name, run_id, ask.as_deref(), &req.replies) {
        return Response::from(&e);
    }
    conversation_snapshot(store, &agent, run_id)
}

/// `GET /api/agents/questions` — every unanswered question across every agent, oldest first: the
/// "needs you" inbox.
///
/// One query over a partial index, so this is cheap enough to sit on a poll. The conversation title
/// costs a lookup apiece, which is bounded by how many questions are actually open — a number that
/// is nearly always nought and never large, because one conversation may only have one.
#[must_use]
pub fn pending_questions(store: &Agents) -> Response {
    let asks = store
        .pending_questions()
        .into_iter()
        .map(|ask| PendingAsk {
            conversation: store
                .get(&ask.agent)
                .ok()
                .flatten()
                .map(|agent| {
                    store
                        .runs(&agent)
                        .into_iter()
                        .find(|r| r.run_id == ask.conv)
                        .map(|r| title_of(&r.message))
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            agent: ask.agent.clone(),
            run_id: ask.conv.clone(),
            ask: agent_ask(&ask),
        })
        .collect();
    ok_json(&PendingAsks { asks })
}

/// `POST /api/agents/goals` — one conversation's goals, or every open goal on the machine when the
/// body names neither an agent nor a run.
///
/// The conversation form answers open *and* closed goals, because what was already met is the
/// context for what is being asked now; the machine-wide form answers only what is still open,
/// which is what a "still going" panel wants.
#[must_use]
pub fn agent_goals(store: &Agents, body: &[u8]) -> Response {
    let req = parse_body::<GoalsOf>(body).unwrap_or_default();
    let (name, run_id) = (req.name.trim(), req.run_id.trim());
    let goals = if name.is_empty() && run_id.is_empty() {
        adi_agents::goals::all_open(store)
    } else {
        adi_agents::goals::of_conversation(store, name, run_id)
    };
    ok_json(&goals_response(&goals))
}

/// `POST /api/agents/goal/set` — write a goal onto a conversation, or reword one that is open.
///
/// Always `human`: this endpoint is the UI, and a run setting its own goal does it through the CLI
/// with its own environment (see [`adi_agents::goals`]). Nothing distinguishes the two afterward
/// except that field, so it must not be taken from the request body.
#[must_use]
pub fn set_agent_goal(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_body::<SetGoal>(body) else {
        return error(
            400,
            "expected JSON body { \"name\": \"…\", \"run_id\": \"…\", \"text\": \"…\", \"goal\"?: \"…\" }",
        );
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let run_id = req.run_id.trim();
    let result = match clean(req.goal) {
        Some(goal_id) => adi_agents::goals::edit(store, &goal_id, &req.text).and_then(|edited| {
            edited.ok_or_else(|| {
                adi_agents::Error::NotFound(format!("no goal “{goal_id}” to reword"))
            })
        }),
        None => adi_agents::goals::create(
            store,
            &agent.name,
            run_id,
            &req.text,
            adi_agents::store::SetBy::Human,
        ),
    };
    match result {
        Ok(_) => ok_json(&goals_response(&adi_agents::goals::of_conversation(
            store,
            &agent.name,
            run_id,
        ))),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/goal/close` — close a goal as met or as given up on.
///
/// A goal somebody else already closed is **not** an error here, exactly as it is not one at the
/// CLI: two people looking at the same chat is the ordinary case, and the second click should show
/// the ending that happened rather than a red box. Only an id that names no goal at all is a 404.
#[must_use]
pub fn close_agent_goal(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_body::<CloseGoal>(body).filter(|r| !r.goal.trim().is_empty()) else {
        return error(
            400,
            "expected JSON body { \"goal\": \"…\", \"as\": \"met\" | \"given_up\", \"note\"?: \"…\" }",
        );
    };
    let goal_id = req.goal.trim();
    let closed = if req.as_.trim() == "met" {
        adi_agents::goals::met(store, goal_id, &req.note)
    } else {
        adi_agents::goals::give_up(store, goal_id, &req.note)
    };
    match closed {
        Ok(adi_agents::store::GoalClosed::Unknown) => {
            error(404, &format!("No goal “{goal_id}”."))
        }
        Ok(adi_agents::store::GoalClosed::Now(goal) | adi_agents::store::GoalClosed::Already(goal)) => {
            ok_json(&goals_response(&adi_agents::goals::of_conversation(
                store, &goal.agent, &goal.conv,
            )))
        }
        Err(e) => Response::from(&e),
    }
}

fn goals_response(goals: &[adi_agents::store::Goal]) -> AgentGoals {
    AgentGoals {
        goals: goals.iter().map(agent_goal).collect(),
    }
}

fn agent_goal(goal: &adi_agents::store::Goal) -> AgentGoal {
    AgentGoal {
        id: goal.id.clone(),
        agent: goal.agent.clone(),
        run_id: goal.conv.clone(),
        text: goal.text.clone(),
        state: goal.state.as_str().to_string(),
        set_by: goal.set_by.as_str().to_string(),
        created_at: goal.created_at,
        nudges: goal.nudges,
        closed_at: goal.closed_at,
        note: goal.note.clone(),
    }
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
            // One question query for the whole answer rather than one per agent: the index is
            // partial and the usual row count is zero, but this endpoint is the chat rail's poll
            // and everything on its path is paid for on every tick.
            let waiting = Waiting::of(store);
            let agents = agents
                .iter()
                .map(|a| runs_response_with(store, a, &waiting))
                .collect();
            ok_json(&AllAgentRuns { agents })
        }
        Err(e) => Response::from(&e),
    }
}

/// Every conversation that is blocked on a person, keyed by the pair that names one.
///
/// Built once and read per run. The alternative — asking per row — is a round trip apiece to
/// answer *nothing pending* for all but a handful of several hundred conversations.
struct Waiting(std::collections::HashMap<(String, String), adi_agents::store::Ask>);

impl Waiting {
    fn of(store: &Agents) -> Self {
        Self(
            store
                .pending_questions()
                .into_iter()
                .map(|ask| ((ask.agent.clone(), ask.conv.clone()), ask))
                .collect(),
        )
    }

    fn get(&self, agent: &str, conv: &str) -> Option<AgentAsk> {
        self.0
            .get(&(agent.to_string(), conv.to_string()))
            .map(agent_ask)
    }
}

/// Build the [`AgentRuns`] history answer for an agent.
fn runs_response(store: &Agents, agent: &StoredAgent) -> AgentRuns {
    runs_response_with(store, agent, &Waiting::of(store))
}

fn runs_response_with(store: &Agents, agent: &StoredAgent, waiting: &Waiting) -> AgentRuns {
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
                pending_question: waiting.get(&agent.name, &r.run_id),
                run_id: r.run_id,
                started_at: r.started_at,
                last_activity: r.last_activity,
                message: title_of(&r.message),
                running: r.running,
                hidden: r.hidden,
                outcome: r.outcome.map(agent_run_outcome),
            })
            .collect(),
    }
}

/// How much of a run's opening task travels with a *listing*.
///
/// Long enough to read as a first paragraph in a tooltip, short enough that four hundred of them
/// are not the answer. An agent's task is routinely a page of instructions, and at 398 sessions
/// that made the cross-agent index 1.4 MB of prompt — re-sent to every connected panel whenever
/// anything in it changed — to fill a rail that truncates each one to 72 characters anyway. The
/// whole message is never lost: it is the conversation's first turn, and the transcript carries it.
const TITLE_MAX: usize = 300;

/// A run's task, cut to [`TITLE_MAX`] characters on a character boundary.
fn title_of(message: &str) -> String {
    if message.chars().count() <= TITLE_MAX {
        return message.to_string();
    }
    let head: String = message.chars().take(TITLE_MAX).collect();
    format!("{head}…")
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
        // Asking is answering in reverse: a backend with no thread to continue has nowhere to
        // deliver an answer into, so there is nothing to derive separately.
        asks: c.answerable,
    }
}

/// Map a stored [`RunOutcome`](adi_agents::store::RunOutcome) onto its wire shape.
fn agent_run_outcome(outcome: adi_agents::store::RunOutcome) -> AgentRunOutcome {
    AgentRunOutcome {
        terminal_reason: outcome.terminal_reason,
        is_error: outcome.is_error,
        cost_micro_usd: outcome.cost_micro_usd,
        duration_ms: outcome.duration_ms,
        num_turns: outcome.num_turns,
        result_head: outcome.result_head,
        noted_at: outcome.noted_at,
    }
}

/// Map a stored [`Ask`](adi_agents::store::Ask) onto the wire shape the card draws itself from.
fn agent_ask(ask: &adi_agents::store::Ask) -> AgentAsk {
    AgentAsk {
        id: ask.id.clone(),
        asked_at: ask.asked_at,
        note: ask.note.clone(),
        questions: ask
            .questions
            .iter()
            .map(|q| AgentQuestion {
                header: q.header.clone(),
                question: q.question.clone(),
                options: q
                    .options
                    .iter()
                    .map(|o| AgentChoice {
                        label: o.label.clone(),
                        description: o.description.clone(),
                    })
                    .collect(),
                multi_select: q.multi_select,
            })
            .collect(),
        deadline: ask.deadline,
        headline: ask.headline(),
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
                adi_agents::ToolStatus::Unanswered => AgentToolStatus::Unanswered,
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
        // Tags, star, and project are omit-to-keep for the reason `bin_tools` and `path` are: the
        // meta setup and the project panel don't offer them, and a save from a form that never
        // showed a field must not be how that field gets taken away.
        tags: match req.tags {
            Some(tags) => tags
                .into_iter()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
            None => stored.as_ref().map(|m| m.tags.clone()).unwrap_or_default(),
        },
        starred: req
            .starred
            .unwrap_or_else(|| stored.as_ref().is_some_and(|m| m.starred)),
        // A blank string still means global — what changed is that *saying nothing* no longer
        // does. The project decides which database, secrets, and knowledge an agent's runs reach,
        // so a save that dropped it did not lose a label, it moved the agent somewhere else.
        project: match req.project {
            Some(project) => clean(Some(project)),
            None => stored.as_ref().and_then(|m| m.project.clone()),
        },
        // The adi tools enabled for this agent (its per-tool checkboxes) — each becomes a shim in
        // the agent's own `.bin` at launch. Trimmed and de-blanked; order + dedup left to the store.
        // Omitted means unchanged, for the reason `path` and `env` below are: a save from a form
        // that never offered the checkboxes would otherwise take every tool away.
        bin_tools: match req.bin_tools {
            Some(ids) => ids
                .into_iter()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
            None => stored
                .as_ref()
                .map(|m| m.bin_tools.clone())
                .unwrap_or_default(),
        },
        // The knowledge bases this agent works with, and whether it keeps one of its own. Both
        // are omit-to-keep for the same reason `bin_tools` is: only the full agent editor offers
        // them, and a save from the meta setup or the project panel must not silently cut an
        // agent off from what it knows.
        knowledge: match req.knowledge {
            Some(bases) => bases
                .into_iter()
                .map(|b| b.trim().to_string())
                .filter(|b| !b.is_empty())
                .collect(),
            None => stored
                .as_ref()
                .map(|m| m.knowledge.clone())
                .unwrap_or_default(),
        },
        memory: req
            .memory
            .unwrap_or_else(|| stored.as_ref().is_some_and(|m| m.memory)),
        // The secrets attached to this agent (its per-secret checkboxes). Only these are decrypted
        // and injected into the agent's runs. A blank scope is normalized to `None` (global).
        // Omit-to-keep, as above: an agent stripped of its credentials by a save that never
        // mentioned them fails at its next run, somewhere else entirely.
        secrets: match req.secrets {
            Some(secrets) => secrets.into_iter().filter_map(secret_attachment).collect(),
            None => stored.as_ref().map(|m| m.secrets.clone()).unwrap_or_default(),
        },
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
        // Omitted means unchanged, for the same reason `path` and `env` are: only the full agent
        // editor offers this checkbox, and a save from the meta setup or the project panel must not
        // quietly re-enable an agent's ability to stop and wait for somebody.
        unattended: req
            .unattended
            .unwrap_or_else(|| stored.as_ref().is_some_and(|m| m.unattended)),
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
        // …and no question either, for the same reason: being blocked on somebody is a property of
        // one conversation, and this snapshot is not of one.
        pending_question: None,
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
        knowledge: m.knowledge,
        memory: m.memory,
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
        unattended: m.unattended,
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

/// The backends whose engine is the Claude CLI/SDK, whatever the executor.
const CLAUDE_BACKENDS: &[&str] = &["pty:claude", "process:claude", "harness:claude-sdk"];

/// The backends whose engine is the Codex CLI.
const CODEX_BACKENDS: &[&str] = &["pty:codex", "process:codex"];

/// The built-in Claude Code tools offered as one-tap toggles on the tool picker. Since the engine's
/// surface is deny-by-default (see `adi_agents`'s `backends::mcp`), this list is a *grant*: an agent
/// gets exactly the tools ticked here, and nothing else the CLI happens to ship.
///
/// These are the bare tool names, verified against `claude --tools <names>` — the CLI accepts each
/// one and advertises exactly those. A scoped specifier (e.g. `Edit(src/**)`) is still typed by hand
/// into the same field. Kept in the order they read best in the picker, not alphabetically.
///
/// `Bash` is deliberately absent, and so are its `BashOutput` / `KillShell` companions: a run's
/// shell is always ADI's own MCP `Bash`, so ticking the engine's would be a toggle that does
/// nothing. The rest of the CLI's surface (cron, claude.ai, worktrees, its own task tracker) is off
/// unless typed in by hand — nothing here needs it, and each is a power an agent never asked for.
const CLAUDE_TOOLS: &[&str] = &[
    "Read",
    "Edit",
    "Write",
    "Glob",
    "Grep",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "Task",
    "Skill",
    "ToolSearch",
    "Workflow",
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
/// `executor:what` pairs — the executor (`pty` / `process` / `harness`) is the run
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
        "Read Edit Write Glob Grep",
        "the engine's built-in tools this agent gets — everything else is off. Empty means ADI's own tools only. Scoped rules like Edit(src/**) work too",
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
        &[ADI_HARNESS, "harness:claude-sdk"],
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

    let mut unattended = agent_field(
        "unattended",
        "Runs unattended",
        AgentFormFieldKind::Checkbox,
    );
    unattended.hint = "nobody is watching this one, so it may not stop to ask — the Ask tool \
                       refuses and tells the run to decide for itself and say what it assumed"
        .into();
    fields.push(unattended);

    let mut tags = agent_field("tags", "Tags", AgentFormFieldKind::Text);
    tags.placeholder = "comma-separated (dispatch / filtering)".into();
    tags.wide = true;
    fields.push(tags);

    let mut tools = field_ids(
        "tools",
        "CLI commands",
        AgentFormFieldKind::Text,
        &[ADI_HARNESS, "harness:claude-sdk"],
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
        ],
        fields,
        presets: setup_presets(),
    }
}

/// The ready-made setups the onboarding wizard offers, in order. Two named routes in — the Claude
/// login most people already have, and an API key anyone can paste — then the manual escape hatch
/// onto every backend and every field.
///
/// Each named preset asks for as little as it can: the credential it cannot work without, and the
/// model, which is the one knob people change on day one. Everything else it pins from what the
/// backend already knows (the provider's endpoint, the variable its key is read from), because a
/// question with one right answer is not a question.
fn setup_presets() -> Vec<AgentSetupPreset> {
    vec![
        AgentSetupPreset {
            id: "claude-sdk".into(),
            label: "Claude Code SDK".into(),
            blurb: "Runs Claude Code headless on the login you already have (Pro / Max), or on an \
                    Anthropic API key."
                .into(),
            backend: "harness:claude-sdk".into(),
            arguments: BTreeMap::new(),
            fields: strings(&["model", "permission_mode"]),
            // The CLI's own login is the usual way in, so a key is the alternative, not a
            // requirement — an empty box here means "use whatever `claude` is already logged in as".
            secret: Some(AgentSetupSecret {
                env: "ANTHROPIC_API_KEY".into(),
                label: "Anthropic API key".into(),
                hint: "Optional — leave blank to use the Claude login on this machine.".into(),
                placeholder: "sk-ant-…".into(),
                required: false,
            }),
            manual: false,
        },
        AgentSetupPreset {
            id: "kimi".into(),
            label: "Kimi API key".into(),
            blurb: "ADI's own agent loop, talking to Moonshot's API with your key. No CLI, no \
                    subscription."
                .into(),
            backend: ADI_HARNESS.into(),
            // The provider is what the loop needs to know; its endpoint and key variable are the
            // provider's own defaults, so they are not asked for and not written. The model is
            // both pinned and asked: the adi loop refuses to run without one, so an empty box
            // would be a saved agent that fails on its first turn.
            arguments: [
                ("provider".to_string(), "monshoot".to_string()),
                ("model".to_string(), "kimi-k2.6".to_string()),
            ]
            .into_iter()
            .collect(),
            fields: strings(&["model"]),
            secret: Some(AgentSetupSecret {
                env: "MOONSHOT_API_KEY".into(),
                label: "Kimi (Moonshot) API key".into(),
                hint: "From platform.moonshot.ai — stored encrypted and injected into this \
                       agent's runs only."
                    .into(),
                placeholder: "sk-…".into(),
                required: true,
            }),
            manual: false,
        },
        AgentSetupPreset {
            id: "manual".into(),
            label: "Manual".into(),
            blurb: "Pick any runtime and set every option yourself — the full agent form, \
                    prefilled."
                .into(),
            backend: String::new(),
            arguments: BTreeMap::new(),
            fields: Vec::new(),
            secret: None,
            manual: true,
        },
    ]
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
/// input, both editing the one space-separated tool spec the run is scoped to.
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
    parse_body::<SaveAgent>(body)
        .filter(|req| !req.name.trim().is_empty() && !req.backend.trim().is_empty())
}

fn bad_save_agent() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"backend\": \"…\", … } with a non-empty name and backend",
    )
}

fn parse_agent_ref(body: &[u8]) -> Option<AgentRef> {
    parse_body::<AgentRef>(body).filter(|req| !req.name.trim().is_empty())
}

fn parse_run_agent(body: &[u8]) -> Option<RunAgent> {
    parse_body::<RunAgent>(body).filter(|req| !req.name.trim().is_empty())
}

fn parse_run_ref(body: &[u8]) -> Option<RunRef> {
    parse_body::<RunRef>(body)
        .filter(|req| !req.name.trim().is_empty() && !req.run_id.trim().is_empty())
}

fn bad_run_ref() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"run_id\": \"…\" } with a non-empty name and run_id",
    )
}

fn parse_hide_run(body: &[u8]) -> Option<HideRun> {
    parse_body::<HideRun>(body)
        .filter(|req| !req.name.trim().is_empty() && !req.run_id.trim().is_empty())
}

fn bad_hide_run() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"run_id\": \"…\", \"hidden\": true } with a non-empty name and run_id",
    )
}

fn parse_reply_to_run(body: &[u8]) -> Option<ReplyToRun> {
    parse_body::<ReplyToRun>(body).filter(|req| {
            !req.name.trim().is_empty()
                && !req.run_id.trim().is_empty()
                && !req.message.trim().is_empty()
    })
}

fn bad_reply_to_run() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"run_id\": \"…\", \"message\": \"…\" } with all three non-empty",
    )
}

fn parse_unqueue_from_run(body: &[u8]) -> Option<UnqueueFromRun> {
    parse_body::<UnqueueFromRun>(body)
        .filter(|req| !req.name.trim().is_empty() && !req.run_id.trim().is_empty())
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
    parse_body::<AgentKeys>(body).filter(|req| {
            !req.name.trim().is_empty() && (!req.text.is_empty() || !req.key.is_empty())
    })
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

    /// The tool checkboxes are omit-to-keep for the same reason the run environment is. This was
    /// once a plain list, so a save from any form that didn't render the checkboxes took every
    /// tool away — the project panel and the onboarding wizard each carried a workaround for it.
    #[test]
    fn a_save_that_omits_the_tools_keeps_them() {
        let store = scratch("tools");
        let with = serde_json::json!({
            "name": "solver", "backend": "pty:claude", "bin_tools": ["sys-tasks", "sys-status"],
        });
        assert_eq!(save_agent(&store, with.to_string().as_bytes()).status, 200);

        // A body from a form that never offered the checkboxes.
        assert_eq!(save_agent(&store, &body(None, None)).status, 200);
        assert_eq!(saved(&store).bin_tools, ["sys-tasks", "sys-status"]);

        // …and stating them empty is how they are actually taken away.
        let none = serde_json::json!({
            "name": "solver", "backend": "pty:claude", "bin_tools": [],
        });
        assert_eq!(save_agent(&store, none.to_string().as_bytes()).status, 200);
        assert!(saved(&store).bin_tools.is_empty());
    }

    /// The knowledge fields are omit-to-keep for the same reason the tool checkboxes are: a save
    /// from a form that never rendered them must not cut the agent off from what it knows, or
    /// take away the memory somebody deliberately gave it.
    #[test]
    fn a_save_that_omits_the_knowledge_fields_keeps_them() {
        let store = scratch("knowledge");
        let with = serde_json::json!({
            "name": "solver", "backend": "pty:claude",
            "knowledge": ["global/runbooks", "agent:reviewer/memory"], "memory": true,
        });
        assert_eq!(save_agent(&store, with.to_string().as_bytes()).status, 200);

        // A body from a form that never offered them.
        assert_eq!(save_agent(&store, &body(None, None)).status, 200);
        let m = saved(&store);
        assert_eq!(m.knowledge, ["global/runbooks", "agent:reviewer/memory"]);
        assert!(m.memory);

        // …and stating them is how they are actually changed.
        let none = serde_json::json!({
            "name": "solver", "backend": "pty:claude", "knowledge": [], "memory": false,
        });
        assert_eq!(save_agent(&store, none.to_string().as_bytes()).status, 200);
        let m = saved(&store);
        assert!(m.knowledge.is_empty());
        assert!(!m.memory);
    }

    /// `unattended` is omit-to-keep for the same reason `path` and `env` are: only the full agent
    /// editor offers the checkbox, and a save from the meta setup or the project panel must not
    /// quietly re-grant an agent the ability to stop and wait for somebody.
    #[test]
    fn a_save_that_omits_unattended_keeps_it() {
        let store = scratch("unattended");
        let on = serde_json::json!({
            "name": "solver", "backend": "pty:claude", "unattended": true,
        });
        assert_eq!(save_agent(&store, on.to_string().as_bytes()).status, 200);
        assert!(saved(&store).unattended);

        // A body from a form that never offered it.
        assert_eq!(save_agent(&store, &body(None, None)).status, 200);
        assert!(saved(&store).unattended, "still unattended");

        // …and stating it false is how it is actually turned off.
        let off = serde_json::json!({
            "name": "solver", "backend": "pty:claude", "unattended": false,
        });
        assert_eq!(save_agent(&store, off.to_string().as_bytes()).status, 200);
        assert!(!saved(&store).unattended);
    }

    /// Tags, star, project and secrets are omit-to-keep for the same reason `unattended` is. The
    /// meta setup and the project panel do not offer them, so they used to be cleared by every save
    /// from those forms — and the project is not a label: it decides which database, secrets, and
    /// knowledge bases an agent's runs reach, so losing it moved the agent somewhere else.
    #[test]
    fn a_save_that_omits_the_filing_fields_keeps_them() {
        let store = scratch("filing");
        let full = serde_json::json!({
            "name": "solver", "backend": "pty:claude",
            "tags": ["bugbounty", "v2"], "starred": true, "project": "bugbounty",
            "secrets": [{ "project": null, "name": "VIRUSTOTAL_API" }],
        });
        assert_eq!(save_agent(&store, full.to_string().as_bytes()).status, 200);

        // A body from a form that never offered any of them — the meta setup's shape.
        let partial = serde_json::json!({ "name": "solver", "backend": "pty:claude" });
        assert_eq!(
            save_agent(&store, partial.to_string().as_bytes()).status,
            200
        );
        let kept = saved(&store);
        assert_eq!(kept.tags, vec!["bugbounty", "v2"]);
        assert!(kept.starred);
        assert_eq!(kept.project.as_deref(), Some("bugbounty"));
        assert_eq!(kept.secrets.len(), 1, "its credentials survived the save");

        // Stating them is still how they are cleared — a blank project is how "global" is said.
        let cleared = serde_json::json!({
            "name": "solver", "backend": "pty:claude",
            "tags": [], "starred": false, "project": "", "secrets": [],
        });
        assert_eq!(
            save_agent(&store, cleared.to_string().as_bytes()).status,
            200
        );
        let now = saved(&store);
        assert!(now.tags.is_empty());
        assert!(!now.starred);
        assert_eq!(now.project, None);
        assert!(now.secrets.is_empty());
    }

    /// Answering into a conversation that is not there is a 404, not a turn: it is what a card
    /// left open in a second tab hits after the chat it belonged to was deleted, and starting a
    /// turn on the strength of a stale card would be the one outcome worse than saying so.
    ///
    /// The *settled-since* case — the question answered a second ago by somebody else — is the
    /// same 404 through the same path, and is pinned where the claim lives (`adi-agents`).
    #[test]
    fn answering_a_conversation_that_is_gone_is_a_404() {
        let store = scratch("answer-404");
        assert_eq!(
            save_agent(
                &store,
                serde_json::json!({ "name": "solver", "backend": "harness:adi" })
                    .to_string()
                    .as_bytes(),
            )
            .status,
            200
        );

        let answer = serde_json::json!({
            "name": "solver", "run_id": "1750000000000-0001", "replies": ["yes"],
        });
        assert_eq!(
            answer_run(&store, answer.to_string().as_bytes()).status,
            404
        );
    }

    /// A malformed body is answered in the shape it should have had, not with a bare 400.
    #[test]
    fn a_malformed_answer_body_says_what_it_wanted() {
        let store = scratch("answer-400");
        let response = answer_run(&store, b"{}");
        assert_eq!(response.status, 400);
    }

    /// The inbox is one query over every agent, and empty is the ordinary answer.
    #[test]
    fn the_question_inbox_answers_even_with_nothing_in_it() {
        let store = scratch("inbox");
        let response = pending_questions(&store);
        assert_eq!(response.status, 200);
        let body: PendingAsks = serde_json::from_str(&response.body).expect("json");
        assert!(body.asks.is_empty());
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

    /// Whether the schema shows `name` for this backend/provider — the same rule the client
    /// renders by, restated here so a preset that names a field nobody would see fails the test
    /// rather than the setup page.
    fn field_applies(spec: &AgentFormSpec, name: &str, backend: &str, provider: &str) -> bool {
        spec.fields.iter().any(|f| {
            f.name == name
                && (f.backend_ids.is_empty() && f.executors.is_empty() && f.providers.is_empty()
                    || f.backend_ids.iter().any(|id| id == backend)
                    || f.executors
                        .iter()
                        .any(|e| Some(e.as_str()) == backend.split(':').next())
                    || (backend == ADI_HARNESS && f.providers.iter().any(|p| p == provider)))
        })
    }

    /// A preset is a promise about the schema: the backend it names is selectable, and every field
    /// it asks for or pins is one the client can actually render for that backend. Nothing in the
    /// wizard checks this at runtime — a stale name would simply render nothing and save nothing.
    #[test]
    fn every_preset_names_a_real_backend_and_real_fields() {
        let spec = agent_form_spec();
        assert!(!spec.presets.is_empty());

        for preset in &spec.presets {
            if preset.manual {
                // The manual preset pins nothing and asks nothing: it *is* the whole schema.
                assert!(preset.backend.is_empty(), "{}", preset.id);
                assert!(preset.fields.is_empty(), "{}", preset.id);
                continue;
            }
            assert!(
                spec.backends.iter().any(|b| b.id == preset.backend),
                "preset {} names unknown backend {}",
                preset.id,
                preset.backend
            );
            let provider = preset.arguments.get("provider").cloned().unwrap_or_default();
            for name in preset.fields.iter().chain(preset.arguments.keys()) {
                assert!(
                    field_applies(&spec, name, &preset.backend, &provider),
                    "preset {} names field {name}, which does not apply to {}",
                    preset.id,
                    preset.backend
                );
            }
        }
    }

    /// The two named ways in, and what each is for: a Claude login that needs no key, and a key
    /// that needs no login. Both are load-bearing — the wizard reads `secret.required` to decide
    /// whether it may save with the box left empty.
    #[test]
    fn the_named_presets_ask_for_the_credential_they_actually_need() {
        let spec = agent_form_spec();
        let preset = |id: &str| {
            spec.presets
                .iter()
                .find(|p| p.id == id)
                .unwrap_or_else(|| panic!("no {id} preset"))
                .clone()
        };

        let claude = preset("claude-sdk");
        assert_eq!(claude.backend, "harness:claude-sdk");
        let key = claude.secret.expect("an API key is offered");
        assert_eq!(key.env, "ANTHROPIC_API_KEY");
        assert!(!key.required, "the CLI login is the other way in");

        let kimi = preset("kimi");
        assert_eq!(kimi.backend, ADI_HARNESS);
        assert_eq!(kimi.arguments.get("provider").map(String::as_str), Some("monshoot"));
        // The loop refuses to start without a model, so the preset arrives with one — in the box,
        // where it can still be changed.
        assert!(kimi.arguments.contains_key("model"), "{:?}", kimi.arguments);
        assert!(kimi.fields.iter().any(|f| f == "model"), "{:?}", kimi.fields);
        let key = kimi.secret.expect("an API key is required");
        assert_eq!(key.env, "MOONSHOT_API_KEY");
        assert!(key.required, "there is no login for this one");
    }
}

// ---- the simulator (a run with a person in the model's seat) ------------------------
//
// Four endpoints over one state shape. Every one of them ends by rendering `sim_state`, so a page
// that stacks a block, ends a turn, or replies gets the prompt back grown by what it just did —
// rather than asking again and drawing something stale in between.
//
// Nothing here composes a prompt, declares a tool, or runs one. `adi_agents` does all three, on the
// paths a real run uses; these handlers are the wire.

/// `POST /api/agents/simulate` — open a run of an agent with a person in the model's seat.
///
/// Always a fresh run. The agent's own environment is materialized exactly as a real launch
/// materializes it — same cwd, same `.bin`, same PATH, same secrets — so the tools the person calls
/// really are the agent's tools.
#[must_use]
pub fn simulate_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_body::<SimulateAgent>(body).filter(|r| !r.name.trim().is_empty()) else {
        return error(
            400,
            "expected JSON body { \"name\": \"…\", \"message\": \"…\" } with a non-empty name",
        );
    };
    let name = req.name.trim();
    let message = req.message.trim();
    if message.is_empty() {
        return error(
            400,
            "A simulated run opens on a task, the same as a real one — enter what the agent is \
             being asked to do.",
        );
    }
    let run_id = match store.simulate(name, message) {
        Ok(adi_agents::Launch::Process { run_id, .. }) => run_id,
        // A pane has no seat to sit in, and `simulate` never opens one — this cannot happen, and
        // saying so beats unwrapping it.
        Ok(adi_agents::Launch::Pty { .. }) => {
            return error(500, "a simulated run should never be a terminal session");
        }
        Err(e) => return Response::from(&e),
    };
    sim_state(store, name, &run_id)
}

/// `POST /api/agents/simulate/prompt` — the run as the model sees it: the composed prompt, its
/// split, the tools it was declared, and the conversation so far.
///
/// The prompt is the one the run *opened with*, read back rather than recomposed. Composing means
/// assembling a spec, which syncs the agent's `.bin` — a write path, and this is polled by a page.
#[must_use]
pub fn simulate_prompt(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_run_ref(body) else {
        return bad_run_ref();
    };
    sim_state(store, req.name.trim(), req.run_id.trim())
}

/// `POST /api/agents/simulate/turn` — close the open turn.
///
/// Every call in it runs, in order, through the same executor the adi loop calls. A failed call is
/// answered with the text the model would have read, not with an error status: that is the tool's
/// answer, and seeing it is the lesson. How the turn ends is decided by the blocks — a call in it
/// means `tool_use` and the seat stays occupied; none means `end_turn` and the run yields.
#[must_use]
pub fn simulate_turn(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_body::<SimulateTurn>(body)
        .filter(|r| !r.name.trim().is_empty() && !r.run_id.trim().is_empty())
    else {
        return error(
            400,
            "expected JSON body { \"name\": \"…\", \"run_id\": \"…\", \"blocks\": [ … ] }",
        );
    };
    let (name, run_id) = (req.name.trim(), req.run_id.trim());
    if req.blocks.is_empty() {
        return error(
            400,
            "A turn has to contain something — say something, call a tool, or both, before ending \
             it.",
        );
    }
    let blocks: Vec<adi_agents::SimBlock> = req.blocks.into_iter().map(sim_block).collect();
    let turn = match store.simulate_turn(name, run_id, &blocks) {
        Ok(turn) => turn,
        Err(e) => return Response::from(&e),
    };
    let results = turn
        .results
        .into_iter()
        .map(|r| AgentSimResult {
            name: r.name,
            output: r.output,
            ok: r.ok,
        })
        .collect();
    match sim_state_of(store, name, run_id) {
        Ok(state) => ok_json(&AgentSimTurn { results, state }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/simulate/reply` — answer a yielded simulated run as yourself.
#[must_use]
pub fn simulate_reply(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_body::<ReplyToRun>(body)
        .filter(|r| !r.name.trim().is_empty() && !r.run_id.trim().is_empty())
    else {
        return error(
            400,
            "expected JSON body { \"name\": \"…\", \"run_id\": \"…\", \"message\": \"…\" }",
        );
    };
    let (name, run_id) = (req.name.trim(), req.run_id.trim());
    let message = req.message.trim();
    if message.is_empty() {
        return error(400, "Nothing to say — type a message before sending it.");
    }
    if let Err(e) = store.simulate_user(name, run_id, message) {
        return Response::from(&e);
    }
    sim_state(store, name, run_id)
}

/// One wire block as the agent layer's own.
fn sim_block(block: AgentSimBlock) -> adi_agents::SimBlock {
    match block {
        AgentSimBlock::Text { text } => adi_agents::SimBlock::Text(text),
        AgentSimBlock::Call { name, input } => adi_agents::SimBlock::Call { name, input },
    }
}

/// The whole state of a simulated run, rendered.
fn sim_state(store: &Agents, name: &str, run_id: &str) -> Response {
    match sim_state_of(store, name, run_id) {
        Ok(state) => ok_json(&state),
        Err(e) => Response::from(&e),
    }
}

fn sim_state_of(
    store: &Agents,
    name: &str,
    run_id: &str,
) -> Result<AgentSimState, AgentStoreError> {
    let agent = get_agent(store, name)?;
    let prompt = store.simulated_prompt(name, run_id)?;

    // The peek answers liveness the same way the run list does, and its transcript is the one the
    // chat renders — no simulated special case on either.
    let peek = store.peek_run(&agent, run_id);
    let turns: Vec<AgentTurn> = store
        .transcript(&agent, run_id)
        .into_iter()
        .map(agent_turn)
        .collect();

    // Tokenized section by section rather than whole, so the ranges are exact by construction: each
    // section is a contiguous slice, so the runs concatenate into one stream.
    //
    // The conversation is tokenized here too, and it has to be: what a turn *appended* — the calls
    // and, above all, their results — is the next thing the model reads, and a prompt view that
    // stopped at the system prompt would show a reader everything except the part their last turn
    // changed. Which is the one part they came to see.
    let mut tokens: Vec<AgentToken> = Vec::new();
    let mut sections: Vec<AgentSimSection> = Vec::new();
    let mut cut = |label: String, text: &str, tokens: &mut Vec<AgentToken>| {
        if text.is_empty() {
            return;
        }
        let from = tokens.len();
        tokens.extend(
            adi_agents::analytics::split(text)
                .into_iter()
                .map(|t| AgentToken {
                    id: t.id,
                    text: t.text,
                    special: t.special,
                }),
        );
        sections.push(AgentSimSection {
            label,
            from,
            to: tokens.len(),
        });
    };
    for section in adi_agents::runner::prompt::sections(&prompt) {
        cut(section.label.to_string(), section.text, &mut tokens);
    }
    for turn in &turns {
        // A queued message has not been asked yet, so it is not in anybody's context.
        if turn.queued {
            continue;
        }
        cut(turn.role.clone(), &turn_text(turn), &mut tokens);
    }
    // Derived, not stored: a run whose seat is empty has yielded, and a run whose seat is taken is
    // mid-loop. Keeping a copy would be a second answer to a question the runner already answers.
    //
    // Three states, and the pair of questions that separates them is "is the seat occupied?" and
    // "has anything been emitted into it?". A seat that has been given up means the last turn was
    // words only. A seat still occupied *after* something was emitted means the last turn called
    // something and the loop came back round. A seat occupied with nothing emitted is a run that
    // has not stopped at all, and has no reason for stopping to report.
    //
    // Emission is read off the timeline rather than off the turn's text, because the turn that
    // called something is still `pending` — its answer is exactly what has not been written yet —
    // so a check for a settled assistant turn reports nothing on the one state this is for.
    //
    // And it is the *open* turn that is asked, not the run: after a person answers as themselves the
    // seat is theirs again with nothing in it, and reporting the previous turn's reason there would
    // put a stale claim above an empty staging area.
    let emitted = turns.last().is_some_and(|t| {
        t.role != "user" && (!t.steps.is_empty() || !t.text.trim().is_empty())
    });
    let stop_reason = if !peek.running {
        adi_agents::STOP_END_TURN.to_string()
    } else if emitted {
        adi_agents::STOP_TOOL_USE.to_string()
    } else {
        String::new()
    };

    Ok(AgentSimState {
        name: agent.name.clone(),
        run_id: run_id.to_string(),
        prompt,
        tokens,
        sections,
        encoding: adi_agents::analytics::ENCODING.to_string(),
        tools: Agents::simulated_tools()
            .into_iter()
            .map(sim_tool)
            .collect(),
        turns,
        stop_reason,
        running: peek.running,
    })
}

/// One declared tool, with its JSON Schema decoded into the fields a form can be built from.
fn sim_tool(tool: adi_agents::ToolDeclaration) -> AgentSimTool {
    let required: Vec<&str> = tool
        .schema
        .get("required")
        .and_then(serde_json::Value::as_array)
        .map(|r| r.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    let props = tool
        .schema
        .get("properties")
        .and_then(serde_json::Value::as_object);
    let field_of = |name: &str| {
        let spec = props
            .and_then(|p| p.get(name))
            .unwrap_or(&serde_json::Value::Null);
        AgentSimField {
            kind: field_kind(name, spec),
            required: required.contains(&name),
            hint: spec
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: name.to_string(),
        }
    };
    // Required first, in the order the tool requires them, then the rest.
    //
    // Not the order the schema literal was written in — that order does not survive: a JSON object
    // decodes into a sorted map, so reading `properties` back gives `background, command,
    // timeout_ms` for `Bash`, which puts a flag nobody sets above the command the call is *about*.
    // The `required` array does survive, and it is the tool's own statement of what matters, so it
    // orders the top of the form and the remainder follows it alphabetically.
    let mut fields: Vec<AgentSimField> = required.iter().map(|name| field_of(name)).collect();
    if let Some(props) = props {
        fields.extend(
            props
                .keys()
                .filter(|name| !required.contains(&name.as_str()))
                .map(|name| field_of(name)),
        );
    }
    AgentSimTool {
        name: tool.name,
        description: tool.description,
        fields,
    }
}

/// Which control a parameter gets.
///
/// The schema type decides four of the five, and cannot decide the fifth: a `string` holding a shell
/// command and a `string` holding a filename are the same type and not the same control. So the
/// ones that are really *bodies* of text are named — by the parameter name, which is the only thing
/// that distinguishes them, and which is stable because it is what the model writes.
fn field_kind(name: &str, spec: &serde_json::Value) -> AgentSimFieldKind {
    match spec.get("type").and_then(serde_json::Value::as_str) {
        Some("boolean") => AgentSimFieldKind::Flag,
        Some("integer" | "number") => AgentSimFieldKind::Number,
        Some("array") => AgentSimFieldKind::List,
        _ if matches!(
            name,
            "command" | "content" | "old_string" | "new_string" | "prompt" | "question"
        ) =>
        {
            AgentSimFieldKind::Text
        }
        _ => AgentSimFieldKind::Line,
    }
}

/// One transcript turn as the model reads it: what was said, and every call it made with what came
/// back.
///
/// The call is written as the tagged block a model actually emits rather than as JSON, on the same
/// reasoning the transcript's own renderer gives: the wire format of some transport is a shape the
/// model never saw, and teaching a reader that shape means they then debug against it.
fn turn_text(turn: &AgentTurn) -> String {
    let mut out = String::new();
    let mut add = |part: &str| {
        if part.trim().is_empty() {
            return;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(part);
    };
    for step in &turn.steps {
        match step {
            AgentStep::Message { text } | AgentStep::Thinking { text } => add(text),
            AgentStep::Tool {
                name,
                input,
                status,
                output,
            } => {
                add(&format!("<invoke name=\"{name}\">{input}</invoke>"));
                // A call still in flight has nothing back yet, and saying so beats an empty result
                // block — which would read as a tool that answered with silence.
                match status {
                    AgentToolStatus::Running => add("<result>(still running)</result>"),
                    _ => add(&format!("<result>\n{output}\n</result>")),
                }
            }
        }
    }
    add(&turn.text);
    out
}
