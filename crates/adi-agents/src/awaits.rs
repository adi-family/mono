//! Awaits — how a run asks to be woken again.
//!
//! A harness turn ends when the model stops calling tools. Everything it was waiting on ends with
//! it: a build that had not finished, a task nobody had created yet, an hour that had not passed.
//! An **await** is the run's own answer to that — a note it leaves for itself saying *wake this
//! conversation when…*, registered with the [`Await`](crate::backends::harness) tool and honored
//! long after the turn that wrote it exited.
//!
//! Three things make one up, and they compose:
//!
//! * **[`events`](Await::events)** — [platform event](adi_events) patterns. A published event whose
//!   name matches is a *candidate* to wake on.
//! * **[`at`](Await::at)** — a deadline. When it passes, that too is a candidate.
//! * **[`check`](Await::check)** — a shell command that decides whether a candidate is really the
//!   moment. Exit 0 wakes the run; anything else leaves the await registered and waiting. This is
//!   the difference between "wake me when *any* task changes" and "wake me when the task I care
//!   about is actually done" — the event says *look now*, the check says *yes, now*.
//!
//! A timer plus a check is a **poll**: a check that says "not yet" re-arms the deadline by
//! [`every`](Await::every), so `every_seconds: 60` with a check is "look every minute until it's
//! true". A candidate with no check wakes immediately.
//!
//! Waking delivers a new user turn into the same conversation, carrying the run's own
//! [`note`](Await::note), what happened, and what the check printed. The next turn replays the whole
//! transcript, so the run comes back with everything it knew — this is a continuation, not a new
//! run. If a turn happens to be in flight the message waits in that conversation's queue, exactly
//! like anything else said mid-answer.
//!
//! It is deliberately *not* [`Agents::reply`](crate::Agents::reply). A reply is a person speaking,
//! and a person speaking into a conversation that has stopped to ask them something is answering
//! it — so a wake sent that way would settle the run's own [question](crate::store::Ask) with this
//! note, and leave whoever it was actually asked of with nothing left to answer. A wake goes
//! through the delivery half alone.
//!
//! **An await fires once.** It is claimed by deleting its record before the reply goes out, so two
//! callers racing on the same await (the event side and the timer side both live in the app) can
//! only wake it between them once. A run that wants to keep watching registers again from the turn
//! it wakes into.
//!
//! Nothing here polls or listens: this crate is the store and the decision. The app owns the clock
//! and feeds it — [`on_event`] when the dispatcher drains a published event, [`tick`] once a second
//! for deadlines and expiry.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;

use adi_config::{Config, now_unix};
use serde::{Deserialize, Serialize};

use crate::Agents;
use crate::backends::harness::tools::wait_with_timeout;
use crate::error::{Error, Result};

/// The store module awaits live under, and each record's extension.
const MODULE: &str = "awaits";
const RECORD_EXT: &str = "json";

/// How many awaits one conversation may have pending. A run that registers a wake per turn without
/// ever consuming one would otherwise fill the store; hitting this is a mistake the model should
/// read about and correct, hence the message rather than a silent drop.
const MAX_PER_CONVERSATION: usize = 8;

/// How long a [`check`](Await::check) may run before it is killed and read as "not yet". Short,
/// because checks run on the app's await worker one after another: a check is a question, not the
/// work.
const CHECK_TIMEOUT_MS: u64 = 20_000;

/// How much of a check's output travels with the wake.
const MAX_CHECK_OUTPUT: usize = 4_000;

/// How long an await lives when its run named no deadline of its own. It exists so an abandoned
/// conversation's awaits don't sit in the store for ever; reaching it drops the await quietly
/// (see [`Await::expiry_wakes`]).
const DEFAULT_LIFETIME_SECS: u64 = 7 * 24 * 60 * 60;

/// A registered wake: who to wake, on what, and how to be sure.
///
/// Written by the `Await` tool from inside a turn, read by the app. Unknown fields are ignored so
/// the record can gain fields without stranding awaits an older build wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Await {
    /// This await's id — also its `<id>.json` file stem, and what the wake message names.
    pub id: String,
    /// The agent whose conversation is woken.
    pub agent: String,
    /// The conversation id to reply into.
    pub conv: String,
    /// The run's note to its future self, handed back verbatim on waking.
    #[serde(default)]
    pub note: String,
    /// Event-name patterns that make a wake a candidate, matched by [`adi_events::matches`] —
    /// `adi.tasks.created`, `adi.tasks.*` (one segment), `adi.**` (the tail).
    #[serde(default)]
    pub events: Vec<String>,
    /// The next deadline, as Unix epoch seconds. Passing it is a candidate, just like an event.
    #[serde(default)]
    pub at: Option<u64>,
    /// How long to wait before looking again when a [`check`](Self::check) says "not yet" — what
    /// turns a deadline into a poll. Set from the request's `every_seconds`, or from its
    /// `after_seconds` when a check is present, since a guarded deadline is a poll however it was
    /// spelled. Without a check there is nothing to re-arm for: the first deadline wakes the run.
    #[serde(default)]
    pub every: Option<u64>,
    /// The command that decides whether a candidate is really the moment: exit 0 wakes the run,
    /// anything else leaves this await registered. Runs in [`cwd`](Self::cwd) with the cause in its
    /// environment; its output travels with the wake.
    #[serde(default)]
    pub check: Option<String>,
    /// Where the check runs — the directory the registering turn was working in, so a relative path
    /// in a check means what it meant to the run that wrote it.
    #[serde(default)]
    pub cwd: String,
    /// When to give up, as Unix epoch seconds.
    #[serde(default)]
    pub expires_at: Option<u64>,
    /// Whether reaching [`expires_at`](Self::expires_at) wakes the run (it asked for a deadline and
    /// deserves to hear that it lapsed) or simply drops the await (it named none, and this is only
    /// [`DEFAULT_LIFETIME_SECS`] tidying up).
    #[serde(default)]
    pub expiry_wakes: bool,
    /// When it was registered, as Unix epoch seconds.
    #[serde(default)]
    pub created_at: u64,
}

impl Await {
    /// Whether a published event named `name` is a candidate for this await.
    #[must_use]
    pub fn wants_event(&self, name: &str) -> bool {
        self.events
            .iter()
            .any(|pattern| adi_events::matches(pattern, name))
    }

    /// A one-line description of what this await is waiting for, for the tool's reply to the model
    /// and for anything that lists pending wakes.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.events.is_empty() {
            parts.push(format!("on {}", self.events.join(", ")));
        }
        if let Some(at) = self.at {
            let secs = at.saturating_sub(now_unix());
            parts.push(match self.every {
                Some(every) => format!("in {secs}s, then every {every}s"),
                None => format!("in {secs}s"),
            });
        }
        if parts.is_empty() {
            parts.push("on nothing".to_string());
        }
        let mut text = parts.join(" or ");
        if self.check.is_some() {
            text.push_str(", if the check passes");
        }
        text
    }
}

/// Why an await is being considered — and, when it wakes, what the run is told happened.
#[derive(Debug, Clone, Copy)]
pub enum Cause<'a> {
    /// A published event matched one of the await's patterns.
    Event {
        /// The event's concrete dotted name.
        name: &'a str,
        /// Its JSON body, as published.
        payload: &'a str,
    },
    /// The await's deadline came due.
    Timer,
    /// The await ran out of time without ever firing.
    Expired,
}

impl Cause<'_> {
    /// The word the check reads as `$ADI_CAUSE`.
    fn tag(&self) -> &'static str {
        match self {
            Self::Event { .. } => "event",
            Self::Timer => "timer",
            Self::Expired => "expired",
        }
    }
}

/// One await that woke its conversation, reported back so the app can log it. `error` is set when
/// the await was claimed but the reply itself failed — the wake is gone either way, which is why it
/// is worth saying out loud.
#[derive(Debug, Clone)]
pub struct Woken {
    /// The await's id.
    pub id: String,
    /// The agent whose conversation was woken.
    pub agent: String,
    /// The conversation the wake was delivered into.
    pub conv: String,
    /// Why it woke.
    pub cause: String,
    /// What went wrong delivering it, if anything.
    pub error: Option<String>,
}

/// The await store: one JSON record per pending wake under `~/.adi/mono/awaits`. Cheap to clone —
/// all state is on disk, like every other store here.
#[derive(Debug, Clone)]
pub struct Awaits {
    config: Config,
}

impl Default for Awaits {
    fn default() -> Self {
        Self::open()
    }
}

impl Awaits {
    /// Open the store backed by the standard config root (`~/.adi/mono`, honoring `$ADI_DIR`).
    #[must_use]
    pub fn open() -> Self {
        Self {
            config: Config::open(),
        }
    }

    /// Open the store backed by a caller-supplied [`Config`] — for tests, or to share the exact
    /// store another subsystem already holds.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// The directory records live in.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.config.module(MODULE).dir().to_path_buf()
    }

    /// The root this store was opened against.
    ///
    /// Exposed for the tools that run beside an await in the same turn — they need the same root to
    /// reach the same event bus, and taking it from here is what stops a second `Config::open()`
    /// pointing a test's tool at the real one.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Every pending await, oldest first. An unreadable or unparseable record is skipped rather
    /// than failing the read: one bad file must not stop every other run from waking.
    #[must_use]
    pub fn list(&self) -> Vec<Await> {
        let Ok(entries) = std::fs::read_dir(self.dir()) else {
            return Vec::new();
        };
        let mut records: Vec<Await> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(RECORD_EXT))
            .filter_map(|p| serde_json::from_slice(&std::fs::read(p).ok()?).ok())
            .collect();
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        records
    }

    /// Every pending await belonging to one conversation.
    #[must_use]
    pub fn for_conversation(&self, agent: &str, conv: &str) -> Vec<Await> {
        self.list()
            .into_iter()
            .filter(|a| a.agent == agent && a.conv == conv)
            .collect()
    }

    /// Write a record, creating or replacing it.
    ///
    /// # Errors
    /// [`Error::Config`] if the record can't be written.
    pub fn save(&self, record: &Await) -> Result<()> {
        let bytes = serde_json::to_vec(record)
            .map_err(|e| Error::Process(format!("couldn't encode an await: {e}")))?;
        self.config
            .module(MODULE)
            .write_raw(&format!("{}.{RECORD_EXT}", record.id), &bytes)?;
        Ok(())
    }

    /// Take an await out of the store, returning whether this call is the one that removed it.
    ///
    /// This is how a wake is *claimed*: the event side and the timer side both live in the app and
    /// can reach the same await at the same moment, and exactly one of them gets `true`.
    #[must_use]
    pub fn claim(&self, id: &str) -> bool {
        self.config
            .module(MODULE)
            .remove_raw(&format!("{id}.{RECORD_EXT}"))
            .unwrap_or(false)
    }

    /// Drop every await belonging to a conversation — what *deleting* one takes with it. Returns how
    /// many were removed.
    ///
    /// Deliberately not what *stopping* a turn does: stopping cuts the answer in flight (and the
    /// queue behind it, which was written expecting that answer), while an await was almost always
    /// registered by an earlier turn that finished. Killing it would silently drop something the run
    /// is still owed.
    #[must_use]
    pub fn forget_conversation(&self, agent: &str, conv: &str) -> usize {
        self.for_conversation(agent, conv)
            .iter()
            .filter(|a| self.claim(&a.id))
            .count()
    }

    /// Drop every await belonging to an agent — what deleting the agent takes with it. Returns how
    /// many were removed.
    #[must_use]
    pub fn forget_agent(&self, agent: &str) -> usize {
        self.list()
            .iter()
            .filter(|a| a.agent == agent)
            .filter(|a| self.claim(&a.id))
            .count()
    }
}

/// What a run asked for, before it becomes a stored [`Await`]. The tool builds one of these from
/// the model's arguments; [`register`] is what turns it into a record.
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// The note handed back on waking.
    pub note: String,
    /// Event patterns to wake on.
    pub events: Vec<String>,
    /// Wake this many seconds from now.
    pub after_seconds: Option<u64>,
    /// Look again this often while a check says "not yet".
    pub every_seconds: Option<u64>,
    /// The command that decides whether it is really time.
    pub check: Option<String>,
    /// Give up after this long, and say so on the way out.
    pub expires_in_seconds: Option<u64>,
    /// Where the check runs — the registering turn's own working directory.
    pub cwd: String,
}

/// Register a wake for `agent`'s conversation `conv`, returning the stored record.
///
/// Rejects a request that names nothing to wake on, and one from a conversation already holding
/// [`MAX_PER_CONVERSATION`] pending awaits. Both come back as [`Error::Arguments`] with a message
/// written for the model that will read it as a failed tool result.
///
/// # Errors
/// [`Error::Arguments`] for a request that can never fire or a conversation over the cap, or
/// [`Error::Config`] if the record can't be written.
pub fn register(store: &Awaits, agent: &str, conv: &str, req: &Request) -> Result<Await> {
    let now = now_unix();
    // A poll's first look is one interval away: `every_seconds` alone means "look every minute",
    // not "look right now and then every minute".
    let at = req
        .after_seconds
        .or(req.every_seconds)
        .map(|secs| now.saturating_add(secs.max(1)));
    if req.events.is_empty() && at.is_none() {
        return Err(Error::Arguments(
            "an await needs something to wake on — give `events`, `after_seconds`, or \
             `every_seconds` (a `check` alone is never looked at)"
                .to_string(),
        ));
    }
    for pattern in &req.events {
        check_pattern(pattern)?;
    }
    let pending = store.for_conversation(agent, conv).len();
    if pending >= MAX_PER_CONVERSATION {
        return Err(Error::Arguments(format!(
            "this conversation already has {pending} awaits pending, which is the limit — wait for \
             one to fire, or narrow what you are waiting on into a single await"
        )));
    }

    let check = req
        .check
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    // A deadline guarded by a check is a poll, whether or not the run spelled it that way. Without
    // this, a check that said "not yet" would leave the deadline sitting in the past and be asked
    // again on every sweep — a one-second poll nobody requested. Looking again after the interval
    // the run itself named is the only reading of "wake me in ten minutes if the build is done"
    // that isn't either a busy loop or silence.
    let every = req
        .every_seconds
        .or_else(|| check.as_ref().and(req.after_seconds))
        .filter(|n| *n > 0);

    let record = Await {
        id: new_id(now),
        agent: agent.to_string(),
        conv: conv.to_string(),
        note: req.note.trim().to_string(),
        events: req.events.iter().map(|e| e.trim().to_string()).collect(),
        at,
        every,
        check,
        cwd: req.cwd.clone(),
        expires_at: Some(
            now.saturating_add(req.expires_in_seconds.unwrap_or(DEFAULT_LIFETIME_SECS).max(1)),
        ),
        expiry_wakes: req.expires_in_seconds.is_some(),
        created_at: now,
    };
    store.save(&record)?;
    Ok(record)
}

/// Reject an event pattern the bus could never match, or one so broad it would wake the run on
/// everything the platform does.
///
/// [`adi_events::matches`] compares segment by segment, so a segment is either a whole wildcard
/// (`*`, `**`) or a literal name — `adi.*.created` is a pattern, `adi.*ed.created` only looks like
/// one and would silently never fire. And a pattern of nothing but wildcards subscribes to the
/// entire bus, including the run's own activity, which is a loop rather than a wait.
fn check_pattern(pattern: &str) -> Result<()> {
    let segments: Vec<&str> = pattern.split('.').collect();
    let well_formed = !pattern.is_empty()
        && segments.iter().all(|seg| {
            *seg == "*" || *seg == "**" || adi_events::validate_name(seg).is_ok()
        });
    if !well_formed {
        return Err(Error::Arguments(format!(
            "{pattern:?} is not an event pattern — every dotted segment is either a name or a whole \
             wildcard (`*` for one segment, `**` for the tail), as in `adi.tasks.*`"
        )));
    }
    if segments.iter().all(|seg| *seg == "*" || *seg == "**") {
        return Err(Error::Arguments(format!(
            "{pattern:?} matches every event the platform publishes, including the ones your own \
             runs cause — name at least one real segment, like `adi.tasks.**`"
        )));
    }
    Ok(())
}

/// Consider every await against a published event, waking the ones whose patterns match and whose
/// check agrees. Called by the app as it drains the event spool.
#[must_use]
pub fn on_event(agents: &Agents, name: &str, payload: &str) -> Vec<Woken> {
    let store = Awaits::with_config(agents.config().clone());
    let cause = Cause::Event { name, payload };
    store
        .list()
        .iter()
        .filter(|a| a.wants_event(name))
        .filter_map(|a| consider(agents, &store, a, cause))
        .collect()
}

/// Consider every await whose deadline has come due, and retire the ones that have run out of time.
/// Called by the app on its own clock — once a second is plenty, since the coarsest thing here is a
/// whole second.
#[must_use]
pub fn tick(agents: &Agents) -> Vec<Woken> {
    let store = Awaits::with_config(agents.config().clone());
    let now = now_unix();
    let mut woken = Vec::new();
    for a in store.list() {
        if a.expires_at.is_some_and(|deadline| now >= deadline) {
            if !a.expiry_wakes {
                let _ = store.claim(&a.id);
                continue;
            }
            if let Some(w) = wake(agents, &store, &a, Cause::Expired, None) {
                woken.push(w);
            }
            continue;
        }
        if a.at.is_some_and(|deadline| now >= deadline)
            && let Some(w) = consider(agents, &store, &a, Cause::Timer)
        {
            woken.push(w);
        }
    }
    woken
}

/// Put one candidate to the await's check, and wake it if the check agrees (or there is none).
///
/// A check that says "not yet" re-arms a polling deadline and leaves everything else alone — the
/// await stays registered, waiting for the next candidate.
fn consider(agents: &Agents, store: &Awaits, a: &Await, cause: Cause<'_>) -> Option<Woken> {
    let checked = a
        .check
        .as_deref()
        .map(|check| run_check(a, check, cause, CHECK_TIMEOUT_MS));
    match checked {
        None => wake(agents, store, a, cause, None),
        Some(outcome) if outcome.passed => wake(agents, store, a, cause, Some(&outcome.output)),
        Some(_) => {
            if let (Cause::Timer, Some(every)) = (cause, a.every) {
                let mut rearmed = a.clone();
                rearmed.at = Some(now_unix().saturating_add(every.max(1)));
                let _ = store.save(&rearmed);
            }
            None
        }
    }
}

/// Claim the await and deliver its wake into the conversation. `None` when someone else claimed it
/// first, which is the whole point of claiming before replying.
fn wake(
    agents: &Agents,
    store: &Awaits,
    a: &Await,
    cause: Cause<'_>,
    check_output: Option<&str>,
) -> Option<Woken> {
    if !store.claim(&a.id) {
        return None;
    }
    let message = wake_message(a, cause, check_output);
    // `deliver`, not `reply`: a reply is a *person* speaking, and a person speaking into a
    // conversation that is waiting on them settles the question it is waiting on. A wake is the
    // platform speaking. Sent through `reply` it would answer the run's own question with this
    // wake note — leaving the person it was actually asked of with nothing to answer.
    let error = agents
        .deliver(&a.agent, &a.conv, &message)
        .err()
        .map(|e| e.to_string());
    Some(Woken {
        id: a.id.clone(),
        agent: a.agent.clone(),
        conv: a.conv.clone(),
        cause: cause.tag().to_string(),
        error,
    })
}

/// The user turn a wake delivers. Written to be read by the model that asked for it: what woke it,
/// what it told itself, and everything the wake carried — because the turn it wrote this in is long
/// over and this message is all the run gets.
fn wake_message(a: &Await, cause: Cause<'_>, check_output: Option<&str>) -> String {
    let mut text = String::new();
    let id = &a.id;
    let checked = a.check.is_some();
    match cause {
        Cause::Event { name, .. } if checked => {
            let _ = writeln!(text, "[await {id} — {name} fired and your check passed]");
        }
        Cause::Event { name, .. } => {
            let _ = writeln!(text, "[await {id} — woken by {name}]");
        }
        Cause::Timer if checked => {
            let _ = writeln!(text, "[await {id} — your check passed]");
        }
        Cause::Timer => {
            let _ = writeln!(text, "[await {id} — the time you asked for]");
        }
        Cause::Expired => {
            let _ = writeln!(
                text,
                "[await {id} — expired without ever firing; nothing you were waiting for happened \
                 in time]"
            );
        }
    }
    if !a.note.is_empty() {
        let _ = write!(text, "\nWhat you asked to be told:\n\n{}\n", a.note);
    }
    if let Cause::Event { payload, .. } = cause
        && !payload.trim().is_empty()
    {
        let _ = write!(text, "\nThe event's payload:\n\n{}\n", truncate(payload));
    }
    if let Some(output) = check_output.map(str::trim).filter(|o| !o.is_empty()) {
        let _ = write!(text, "\nWhat your check printed:\n\n{output}\n");
    }
    text.push_str(
        "\nThis await is spent. Register another one if you still need to be woken later.",
    );
    text
}

/// What running an await's check produced.
struct CheckOutcome {
    /// Whether it exited 0 — the whole verdict.
    passed: bool,
    /// Its combined output, capped, for the wake message.
    output: String,
}

/// Run an await's check in the directory the registering turn worked in, telling it why it is being
/// asked. A check that fails to start, or outstays `timeout_ms`, reads as "not yet": the await keeps
/// waiting rather than waking a run on a question that was never answered.
///
/// The budget is a parameter only so a test can prove the kill path in milliseconds instead of
/// [`CHECK_TIMEOUT_MS`]; every real call passes that constant.
fn run_check(a: &Await, check: &str, cause: Cause<'_>, timeout_ms: u64) -> CheckOutcome {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(check);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(check);
        c
    };
    if !a.cwd.trim().is_empty() {
        cmd.current_dir(&a.cwd);
    }
    cmd.env("ADI_AWAIT", &a.id)
        .env("ADI_CAUSE", cause.tag())
        .env("ADI_NOTE", &a.note);
    if let Cause::Event { name, payload } = cause {
        cmd.env("ADI_EVENT", name).env("ADI_PAYLOAD", payload);
    }

    match wait_with_timeout(cmd, timeout_ms) {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.trim().is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            CheckOutcome {
                passed: output.status.success(),
                output: truncate(&text),
            }
        }
        Err(e) => CheckOutcome {
            passed: false,
            output: e,
        },
    }
}

/// Cut over-long text down for the wake message, saying so where the model will see it.
fn truncate(text: &str) -> String {
    if text.len() <= MAX_CHECK_OUTPUT {
        return text.to_string();
    }
    let mut cut = MAX_CHECK_OUTPUT;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n[… {} more bytes]", &text[..cut], text.len() - cut)
}

/// A fresh await id: the second it was registered, this process, and a counter — sorts
/// chronologically, is unique across concurrent turns, and is safe as a filename.
fn new_id(now: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("w-{now:010}-{:06}-{seq:04}", std::process::id() % 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Awaits {
        let root = std::env::temp_dir().join(format!(
            "adi-awaits-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Awaits::with_config(Config::with_root(root))
    }

    fn request(note: &str) -> Request {
        Request {
            note: note.into(),
            events: vec!["adi.tasks.*".into()],
            ..Request::default()
        }
    }

    #[test]
    fn a_registered_await_round_trips_and_is_claimed_exactly_once() {
        let store = scratch("roundtrip");
        let saved = register(&store, "watcher", "conv-1", &request("check the deploy")).expect("register");
        assert!(saved.id.starts_with("w-"));

        let listed = store.for_conversation("watcher", "conv-1");
        assert_eq!(listed, vec![saved.clone()]);
        assert!(store.for_conversation("watcher", "other").is_empty());

        assert!(store.claim(&saved.id));
        assert!(!store.claim(&saved.id));
        assert!(store.list().is_empty());

        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn an_await_that_can_never_fire_is_refused_with_a_reason() {
        let store = scratch("nothing");
        let req = Request {
            note: "waiting".into(),
            check: Some("true".into()),
            ..Request::default()
        };
        let err = register(&store, "watcher", "conv-1", &req).expect_err("must be refused");
        assert!(
            matches!(&err, Error::Arguments(m) if m.contains("something to wake on")),
            "{err}"
        );
        assert!(store.list().is_empty(), "nothing was stored");

        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_conversation_cannot_hoard_awaits() {
        let store = scratch("cap");
        for i in 0..MAX_PER_CONVERSATION {
            register(&store, "watcher", "conv-1", &request(&format!("note {i}"))).expect("register");
        }
        let err = register(&store, "watcher", "conv-1", &request("one too many"))
            .expect_err("the cap must bite");
        assert!(matches!(&err, Error::Arguments(m) if m.contains("limit")), "{err}");
        register(&store, "watcher", "conv-2", &request("elsewhere")).expect("other conversation");

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The two patterns worth refusing: one the bus could never match, and one that matches
    /// everything — including the events a run's own wake goes on to cause.
    #[test]
    fn a_pattern_that_cannot_match_or_matches_everything_is_refused() {
        for good in ["adi.tasks.created", "adi.tasks.*", "adi.**", "*.tasks.created"] {
            assert!(check_pattern(good).is_ok(), "{good} should be a pattern");
        }
        for bad in ["", "adi.*ed.created", "adi tasks.*", "adi/tasks"] {
            let err = check_pattern(bad).expect_err("{bad} should be refused");
            assert!(
                matches!(&err, Error::Arguments(m) if m.contains("whole wildcard")),
                "{bad}: {err}"
            );
        }
        for everything in ["**", "*", "*.**"] {
            let err = check_pattern(everything).expect_err("too broad");
            assert!(
                matches!(&err, Error::Arguments(m) if m.contains("every event")),
                "{everything}: {err}"
            );
        }
    }

    #[test]
    fn patterns_decide_which_events_are_candidates() {
        let store = scratch("patterns");
        let req = Request {
            note: "tasks only".into(),
            events: vec!["adi.tasks.*".into(), "adi.agents.run.**".into()],
            ..Request::default()
        };
        let saved = register(&store, "watcher", "conv-1", &req).expect("register");

        assert!(saved.wants_event("adi.tasks.created"));
        assert!(saved.wants_event("adi.agents.run.started"));
        assert!(!saved.wants_event("adi.tasks.sub.created"));
        assert!(!saved.wants_event("adi.projects.saved"));

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A poll is a deadline plus a check: `every_seconds` sets the *first* look one interval out,
    /// so registering one never fires it immediately.
    #[test]
    fn a_poll_looks_one_interval_from_now_and_keeps_its_interval() {
        let store = scratch("poll");
        let req = Request {
            note: "is the build done?".into(),
            every_seconds: Some(60),
            check: Some("test -f build.done".into()),
            ..Request::default()
        };
        let saved = register(&store, "watcher", "conv-1", &req).expect("register");
        let now = now_unix();
        assert!(saved.at.is_some_and(|at| at > now), "the first look is in the future");
        assert!(saved.at.is_some_and(|at| at <= now + 61));
        assert_eq!(saved.every, Some(60));
        assert!(saved.describe().contains("then every 60s"), "{}", saved.describe());

        let guarded = register(
            &store,
            "watcher",
            "conv-2",
            &Request {
                note: "still building?".into(),
                after_seconds: Some(600),
                check: Some("test -f build.done".into()),
                ..Request::default()
            },
        )
        .expect("register");
        assert_eq!(guarded.every, Some(600));

        let once = register(
            &store,
            "watcher",
            "conv-3",
            &Request {
                note: "in ten minutes".into(),
                after_seconds: Some(600),
                ..Request::default()
            },
        )
        .expect("register");
        assert_eq!(once.every, None);

        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_check_that_exits_zero_passes_and_carries_its_output() {
        let store = scratch("check");
        let a = register(
            &store,
            "watcher",
            "conv-1",
            &Request {
                note: "n".into(),
                events: vec!["adi.tasks.*".into()],
                check: Some("echo the-build-is-green".into()),
                ..Request::default()
            },
        )
        .expect("register");

        let outcome = run_check(
            &a,
            a.check.as_deref().expect("check"),
            Cause::Event {
                name: "adi.tasks.created",
                payload: "{}",
            },
            CHECK_TIMEOUT_MS,
        );
        assert!(outcome.passed);
        assert!(outcome.output.contains("the-build-is-green"), "{}", outcome.output);

        assert!(!run_check(&a, "exit 1", Cause::Timer, CHECK_TIMEOUT_MS).passed);

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The check is told why it was asked, so one command can serve a poll and an event.
    #[test]
    fn a_check_can_read_the_cause_and_the_event_that_woke_it() {
        let store = scratch("checkenv");
        let a = register(
            &store,
            "watcher",
            "conv-1",
            &Request {
                note: "n".into(),
                events: vec!["adi.tasks.*".into()],
                ..Request::default()
            },
        )
        .expect("register");

        let script = r#"printf '%s/%s/%s' "$ADI_CAUSE" "$ADI_EVENT" "$ADI_PAYLOAD""#;
        let outcome = run_check(
            &a,
            script,
            Cause::Event {
                name: "adi.tasks.created",
                payload: r#"{"id":"t1"}"#,
            },
            CHECK_TIMEOUT_MS,
        );
        assert_eq!(outcome.output, r#"event/adi.tasks.created/{"id":"t1"}"#);

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A check that hangs must not hold the app's await worker, and must read as "not yet" rather
    /// than waking a run on a question that was never answered. A tiny budget, so the test costs the
    /// timeout rather than the sleep.
    #[test]
    fn a_hanging_check_is_killed_and_reads_as_not_yet() {
        let a = Await {
            id: "w-test".into(),
            agent: "watcher".into(),
            conv: "conv-1".into(),
            note: String::new(),
            events: Vec::new(),
            at: None,
            every: None,
            check: Some("sleep 30".into()),
            cwd: String::new(),
            expires_at: None,
            expiry_wakes: false,
            created_at: 0,
        };
        let outcome = run_check(&a, "sleep 30", Cause::Timer, 200);
        assert!(!outcome.passed, "a killed check must never wake a run");
        assert!(outcome.output.contains("still running"), "{}", outcome.output);
    }

    #[test]
    fn the_wake_message_carries_the_note_the_event_and_the_check() {
        let a = Await {
            id: "w-42".into(),
            agent: "watcher".into(),
            conv: "conv-1".into(),
            note: "check whether the deploy landed".into(),
            events: vec!["adi.tasks.*".into()],
            at: None,
            every: None,
            check: Some("true".into()),
            cwd: String::new(),
            expires_at: None,
            expiry_wakes: false,
            created_at: 0,
        };
        let text = wake_message(
            &a,
            Cause::Event {
                name: "adi.tasks.created",
                payload: r#"{"id":"t1"}"#,
            },
            Some("build green"),
        );
        assert!(text.contains("adi.tasks.created fired and your check passed"), "{text}");
        assert!(text.contains("check whether the deploy landed"), "{text}");
        assert!(text.contains(r#"{"id":"t1"}"#), "{text}");
        assert!(text.contains("build green"), "{text}");
        assert!(text.contains("spent"), "the run must know it has to re-register: {text}");

        let expired = wake_message(&a, Cause::Expired, None);
        assert!(expired.contains("expired"), "{expired}");

        let polled = wake_message(&a, Cause::Timer, Some("build green"));
        assert!(polled.contains("your check passed"), "{polled}");
        assert!(!polled.contains("the time you asked for"), "{polled}");
        let bare = Await {
            check: None,
            ..a.clone()
        };
        assert!(
            wake_message(&bare, Cause::Timer, None).contains("the time you asked for"),
            "a checkless timer is the clock and nothing else"
        );
    }

    /// The form the tool's description now leads with: a script on a schedule, no events at all.
    /// It has to survive registration — an await with no `events` is not an award with nothing to
    /// wake on, because its timer is the candidate and its check is the verdict.
    #[test]
    fn a_check_on_a_schedule_needs_no_events() {
        let store = scratch("scriptonly");
        let saved = register(
            &store,
            "watcher",
            "conv-1",
            &Request {
                note: "wait for the build".into(),
                every_seconds: Some(30),
                check: Some("/tmp/is-the-build-ready.sh".into()),
                ..Request::default()
            },
        )
        .expect("a check on a schedule is a complete await");

        assert!(saved.events.is_empty(), "no events, and none needed");
        assert!(saved.at.is_some(), "its timer is what makes it a candidate");
        assert_eq!(saved.every, Some(30), "and it looks again while the check says no");
        assert!(!saved.wants_event("adi.tasks.created"), "it subscribes to nothing");

        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn forgetting_a_conversation_takes_only_its_own_awaits() {
        let store = scratch("forget");
        register(&store, "watcher", "conv-1", &request("a")).expect("register");
        register(&store, "watcher", "conv-1", &request("b")).expect("register");
        register(&store, "watcher", "conv-2", &request("c")).expect("register");

        assert_eq!(store.forget_conversation("watcher", "conv-1"), 2);
        assert!(store.for_conversation("watcher", "conv-1").is_empty());
        assert_eq!(store.for_conversation("watcher", "conv-2").len(), 1);

        let _ = std::fs::remove_dir_all(store.dir());
    }
}
