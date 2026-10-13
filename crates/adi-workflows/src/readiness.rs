//! Deciding whether a waiting run is worth a fresh `bun` attempt this sweep — ADI-MONO-150's fix
//! for what [`crate::Engine::run_tick`] used to do: attempt *every* unfinished run on *every*
//! sweep, however long a run has been sitting on a slow person task or a far-off `ctx.sleep`. A
//! proof-post run waiting 48h on a review would otherwise cost ~170,000 `bun` spawns for nothing.
//!
//! # Why this is a cache, not stored status
//!
//! Every signal [`Readiness::due`] reads — a run's own tasks, the runs it has caused, its pending
//! checkpoints, the event log's position — comes straight from the real stores, fresh, every time
//! a decision actually matters. What's remembered here (a fingerprint of those signals, and when a
//! run was last attempted) only ever answers "is it worth spawning `bun` to find out again right
//! now" — never "what state is this run in". Losing it (a process restart; it's never persisted)
//! costs exactly one extra attempt per run on the very next sweep, not a wrong answer: every run
//! simply looks never-attempted again, which [`Readiness::due`] already treats as due.
//!
//! The signals a waiting run can change by, and what each becomes here:
//! - a task with `workflow_run` = this run changed → each task's own `status`/`result`/
//!   `give_up_reason` ([`adi_tasks::Tasks::workflow_run_signature`]) — this also covers the run's
//!   own root task, so completing, giving up on, or (via [`crate::rewind`]) reopening it already
//!   shows up without a separate case; and it covers "just rewound", since a rewind always
//!   reopens the root task.
//! - a child run finished → [`crate::store::WorkflowStore::children_signature`].
//! - the event log advanced past a pending `ctx.waitFor` → folded in only when a pending
//!   checkpoint actually recorded a `since_seq` (see `attempt.rs`'s `on_wait_for`).
//! - a pending sleep/timeout deadline passed → read directly from [`crate::store::StepRecord`]'s
//!   own pending markers (`until_ms`, or `started_at_ms` + `timeout_ms`) and checked against the
//!   wall clock on every call, never cached as a fixed instant to later question.
//! - "just poured" → a `run_key` [`Readiness::due`] has never seen (or whose entry was cleared
//!   when the run last finished) has no cached fingerprint, so it is due unconditionally.
//!
//! A safety interval ([`SAFETY_INTERVAL`]) re-attempts a run regardless of all of the above, in
//! case a signal this cache doesn't model (or a bug in it) silently missed a real change.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use adi_events::Events;
use adi_tasks::Tasks;

use crate::store::{RunRow, WorkflowStore};

/// How long a run that looks unchanged still goes before being re-attempted anyway — the spec's
/// own "also re-attempt on a safety interval (e.g. 5 min) in case a signal was missed."
pub(crate) const SAFETY_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// What's remembered about one run's last attempt — see the module doc for why this is purely a
/// performance cache.
#[derive(Debug)]
struct Entry {
    fingerprint: u64,
    last_attempt: Instant,
}

/// The per-run attempt-worthiness cache — one per [`crate::Engine`], shared (behind an `Arc`, on
/// `Engine` itself) across every clone, so the dedicated attempt worker and anything else calling
/// the same engine agree on what's already been tried.
#[derive(Debug)]
pub(crate) struct Readiness {
    safety_interval: Duration,
    entries: Mutex<HashMap<String, Entry>>,
}

impl Readiness {
    pub(crate) fn new() -> Self {
        Self::with_safety_interval(SAFETY_INTERVAL)
    }

    /// For tests only — a real 5-minute wait is not something a test should sit through to prove
    /// the safety interval actually fires.
    pub(crate) fn with_safety_interval(safety_interval: Duration) -> Self {
        Self {
            safety_interval,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// `runs`, filtered down to the ones actually worth a `bun` attempt right now — see the module
    /// doc for what "worth it" means. Order is preserved, so a caller that cares about "oldest
    /// unfinished run first" (as [`crate::store::WorkflowStore::list_unfinished`] already
    /// promises) still gets that.
    pub(crate) fn due(
        &self,
        tasks: &Tasks,
        store: &WorkflowStore,
        events: &Events,
        runs: &[RunRow],
    ) -> Vec<RunRow> {
        let now = Instant::now();
        let now_ms = crate::now_ms();
        let entries = self.lock();
        runs.iter()
            .filter(|run| match entries.get(&run.run_key) {
                None => true,
                Some(entry) => {
                    let signal = Signal::of(tasks, store, events, run);
                    signal.fingerprint != entry.fingerprint
                        || signal.deadline_ms.is_some_and(|deadline| now_ms >= deadline)
                        || now.duration_since(entry.last_attempt) >= self.safety_interval
                }
            })
            .cloned()
            .collect()
    }

    /// Call once right after attempting `run`, whatever the outcome — refreshes this run's cached
    /// memory to match the state the attempt actually left behind (never the state from before
    /// it), or drops the entry entirely once the run has reached a terminal state, since a
    /// finished run is never checked again (and, were it later rewound, ought to look exactly like
    /// a run `due` has never seen).
    pub(crate) fn record_attempt(
        &self,
        tasks: &Tasks,
        store: &WorkflowStore,
        events: &Events,
        run: &RunRow,
        finished: bool,
    ) {
        let mut entries = self.lock();
        if finished {
            entries.remove(&run.run_key);
            return;
        }
        let signal = Signal::of(tasks, store, events, run);
        entries.insert(
            run.run_key.clone(),
            Entry {
                fingerprint: signal.fingerprint,
                last_attempt: Instant::now(),
            },
        );
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// What a run looks like right now: a hash of everything [`Readiness::due`] treats as "changed"
/// (never read back out except to compare against an earlier `Signal`'s own hash, so what's
/// actually hashed only has to be *consistent*, never meaningful on its own), plus the earliest
/// deadline a pending checkpoint has told the host about so far, if any.
struct Signal {
    fingerprint: u64,
    deadline_ms: Option<u64>,
}

impl Signal {
    fn of(tasks: &Tasks, store: &WorkflowStore, events: &Events, run: &RunRow) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();

        // A read failure here (store unreachable, mid-migration, …) must never silently wedge a
        // run — folding in an empty signature just means this sweep compares against "no tasks",
        // which is wrong at most in the direction of "attempt it", never "skip it forever".
        for (id, status, result, give_up_reason) in
            tasks.workflow_run_signature(&run.run_key).unwrap_or_default()
        {
            id.hash(&mut hasher);
            status.as_str().hash(&mut hasher);
            result.map(|v| v.to_string()).hash(&mut hasher);
            give_up_reason.hash(&mut hasher);
        }
        store
            .children_signature(&run.run_key)
            .unwrap_or((0, 0))
            .hash(&mut hasher);

        let mut deadline_ms = None;
        let mut waiting_on_events = false;
        for (step_key, state) in store.list_pending(&run.run_key).unwrap_or_default() {
            step_key.hash(&mut hasher);
            state.to_string().hash(&mut hasher);
            if state.get("since_seq").is_some() {
                waiting_on_events = true;
            }
            deadline_ms = min_deadline(deadline_ms, state.get("until_ms").and_then(serde_json::Value::as_u64));
            let task_deadline = match (
                state.get("started_at_ms").and_then(serde_json::Value::as_u64),
                state.get("timeout_ms").and_then(serde_json::Value::as_u64),
            ) {
                (Some(started), Some(timeout)) => Some(started.saturating_add(timeout)),
                _ => None,
            };
            deadline_ms = min_deadline(deadline_ms, task_deadline);
        }
        // Only folded in when something is actually blocked on the log's position — otherwise
        // every run in the system would look "changed" on every event anyone emits.
        if waiting_on_events {
            events.head().unwrap_or(0).hash(&mut hasher);
        }

        Self {
            fingerprint: hasher.finish(),
            deadline_ms,
        }
    }
}

fn min_deadline(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(v), None) | (None, Some(v)) => Some(v),
        (None, None) => None,
    }
}
