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
//! # An action can register one for you
//!
//! A run does not have to think of this itself. Any action that *starts* something and returns
//! before it ends — launching another agent, most obviously — can call [`follow_up`] and register
//! the wake on its caller's behalf, then say so in what it prints. The run reads "it is running,
//! and you will be told when it ends" instead of a run id it would otherwise have to remember to go
//! back and poll. [`caller`] is what makes that possible: `ADI_AGENT` and `ADI_RUN_ID` are in the
//! environment of every command a turn runs, so a command knows which conversation to wake without
//! being passed anything.
//!
//! Such a wake must be exact, which is what [`when`](Await::when) is for: `adi.agents.run.finished`
//! fires for every run on the machine, and an await matching the name alone would wake its
//! conversation on a stranger's ending. Filtering the payload down to the one `run_id` is the
//! difference between a wake and a false alarm.
//!
//! And it must be refusable — a wake the run never asked for cannot be one it is stuck with. So a
//! pending await can be dropped ([`ignore`]) or changed in place ([`update`]), both scoped to the
//! conversation that owns it. Changing beats replacing: an await rewritten in place never stops
//! watching, where dropping one and registering another leaves a gap the thing being waited on can
//! finish inside.
//!
//! Nothing here polls or listens: this crate is the store and the decision. The app owns the clock
//! and feeds it — [`on_event`] when the dispatcher drains a published event, [`tick`] once a second
//! for deadlines and expiry.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;

use adi_config::{Config, now_unix};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    /// Payload fields a matching event must *all* carry for it to be a candidate — the difference
    /// between "wake me when a run finishes" and "wake me when **this** run finishes".
    ///
    /// Compared against the event's top-level JSON fields as text, so `run_id` reads a string and
    /// `is_error` reads `true`. Only events are filtered: a timer carries no payload, and a deadline
    /// comes due whatever is written here.
    #[serde(default)]
    pub when: BTreeMap<String, String>,
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

    /// Whether a published event is a candidate: its name matches a pattern *and* its payload
    /// carries every field [`when`](Self::when) names.
    ///
    /// The payload half is what lets an action register an exact wake on its caller's behalf.
    /// `adi.agents.run.finished` is published for every run on the machine, so an await matching the
    /// name alone would wake a conversation on a stranger's ending and it would read it as its own.
    #[must_use]
    pub fn wants(&self, name: &str, payload: &str) -> bool {
        self.wants_event(name) && self.payload_matches(payload)
    }

    /// Whether `payload` carries every field [`when`](Self::when) names.
    ///
    /// A payload that is not a JSON object matches *nothing* rather than everything: a filter that
    /// silently stops filtering is worse than one that never fires, because the run reads whatever
    /// arrives as the thing it asked for.
    fn payload_matches(&self, payload: &str) -> bool {
        if self.when.is_empty() {
            return true;
        }
        let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(payload) else {
            return false;
        };
        self.when.iter().all(|(field, want)| {
            fields.get(field).is_some_and(|found| match found {
                Value::String(text) => text == want,
                other => other.to_string() == *want,
            })
        })
    }

    /// A one-line description of what this await is waiting for, for the tool's reply to the model
    /// and for anything that lists pending wakes.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.events.is_empty() {
            let mut on = format!("on {}", self.events.join(", "));
            if !self.when.is_empty() {
                let fields: Vec<String> = self
                    .when
                    .iter()
                    .map(|(field, value)| format!("{field}={value}"))
                    .collect();
                let _ = write!(on, " carrying {}", fields.join(", "));
            }
            parts.push(on);
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
        let mut records: Vec<Await> = self
            .config
            .module(MODULE)
            .raw_paths_with_ext(RECORD_EXT)
            .unwrap_or_default()
            .into_iter()
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

    /// One pending await by id, or `None` if it is not (or no longer) pending.
    ///
    /// Found by scanning rather than by opening `<id>.json`, so an id that arrived from outside
    /// cannot name a path — the store is small and its records are already read whole.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Await> {
        self.list().into_iter().find(|a| a.id == id)
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
    /// Payload fields a matching event must carry — see [`Await::when`].
    pub when: BTreeMap<String, String>,
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
    if !req.when.is_empty() && req.events.is_empty() {
        return Err(Error::Arguments(
            "`when` filters an event's payload, and this await waits on no events — give `events` \
             too, or drop it"
                .to_string(),
        ));
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
        when: req.when.clone(),
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

/// The conversation a command was invoked *from*, read out of the environment every turn gives its
/// commands.
///
/// `None` for a person at a terminal, a trigger, or the app itself — none of them has a transcript
/// to be woken into, and an action that guessed at one would leave a wake nobody ever reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The agent whose run this is.
    pub agent: String,
    /// The conversation the run is speaking in.
    pub conv: String,
}

/// Who is running this command, if it is a run at all. See [`Caller`].
#[must_use]
pub fn caller() -> Option<Caller> {
    let named = |key: &str| {
        std::env::var(key)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    Some(Caller {
        agent: named(crate::workspace::AGENT_ENV)?,
        conv: named(crate::workspace::CONV_ENV)?,
    })
}

/// Register a wake on behalf of the run whose command this is, and hand back what that run needs to
/// read about it.
///
/// This is how an action that *starts* something long-running stops being a dead end. Launching an
/// agent, filing work, kicking off a build — each returns in milliseconds and finishes minutes
/// later, and until now the only ways to hear the ending were to block the whole turn on it or to
/// come back and poll. An action that calls this instead answers the caller with "it is running,
/// and you will be told" — the wake is already registered by the time the tool result is read.
///
/// Three properties it is worth being explicit about:
///
/// * **It only ever speaks to a run.** [`caller`] is the gate, and the action asks it before
///   reaching here — a person who typed the command is told nothing about a wake they could never
///   be woken by.
/// * **It never fails the action.** The thing was started; a wake that could not be registered is
///   reported as exactly that, in the same breath, so the run knows to check on it itself. Refusing
///   the launch over a bookkeeping record would be the worse of the two failures — the same
///   reasoning the background-job path is written under.
/// * **The run can refuse it.** The message names how to drop it and how to change it, because a
///   wake the caller did not ask for must be as easy to be rid of as it was to receive — and it
///   holds one of the few slots the conversation gets until it is.
#[must_use]
pub fn follow_up(store: &Awaits, who: &Caller, req: &Request) -> String {
    let mut req = req.clone();
    if req.cwd.trim().is_empty() {
        req.cwd = std::env::current_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
    }
    match register(store, &who.agent, &who.conv, &req) {
        Ok(registered) => format!(
            "Registered await {id} for you — you will be woken here {what}. Finish your turn; \
             don't poll for it.\n  don't want it:  adi-mono agents awaits ignore {id}\n  change \
             it:      adi-mono agents awaits update {id} --note '…'",
            id = registered.id,
            what = registered.describe(),
        ),
        // The work is already running and only the wake is missing. Say which is which, the way a
        // background job does, so the run corrects the right half.
        Err(e) => format!(
            "Nothing will wake you when this ends: {e}\nIt is running either way — look in on it \
             yourself, or register your own wake once you have room."
        ),
    }
}

/// Drop a pending await of `agent`'s conversation `conv`, returning the record that is now gone.
///
/// Scoped to the conversation on purpose. Every await in the store is one directory apart and an id
/// travels in plain text, so without the scope one run could quietly cancel another's wake and leave
/// it waiting on something that is never coming.
///
/// # Errors
/// [`Error::Arguments`] when this conversation has no such await pending — the message lists the
/// ones it does have, since a run reaching for the wrong id usually holds a stale one.
pub fn ignore(store: &Awaits, agent: &str, conv: &str, id: &str) -> Result<Await> {
    let found = mine(store, agent, conv, id)?;
    if !store.claim(&found.id) {
        return Err(Error::Arguments(format!(
            "await {id} fired before this reached it — there is nothing left to ignore, and the \
             wake is already on its way into this conversation"
        )));
    }
    Ok(found)
}

/// What an [`update`] changes about a pending await. Every field is omit-to-keep: `None` leaves
/// that part of the record exactly as it was, so changing a note says nothing about a check.
#[derive(Debug, Clone, Default)]
pub struct Change {
    /// Replace the note handed back on waking.
    pub note: Option<String>,
    /// Replace the event patterns outright.
    pub events: Option<Vec<String>>,
    /// Replace the payload fields a matching event must carry.
    pub when: Option<BTreeMap<String, String>>,
    /// Move the next deadline to this many seconds from now.
    pub after_seconds: Option<u64>,
    /// Replace the polling interval.
    pub every_seconds: Option<u64>,
    /// Replace the check. An empty string removes it, which is the only way to say "stop asking and
    /// just wake me".
    pub check: Option<String>,
    /// Move the giving-up point to this many seconds from now, and wake the run when it arrives.
    pub expires_in_seconds: Option<u64>,
}

/// Change a pending await of `agent`'s conversation `conv` in place, returning the record as it now
/// stands.
///
/// The point is the one an automatically registered wake raises: an action decided what the run
/// would be told and when, and the run may know better. Widening a check, moving a deadline out, or
/// rewriting the note so the woken turn reads the *reason* rather than the mechanics — all of it
/// beats dropping the await and registering a replacement, which loses the wake in the gap.
///
/// The record is claimed before it is rewritten, so an await that fires mid-change is reported as
/// spent rather than quietly resurrected by the save.
///
/// # Errors
/// [`Error::Arguments`] when no such await is pending here, when the change leaves it with nothing
/// to wake on, or when it names a pattern the bus could never match; [`Error::Config`] if the
/// rewritten record can't be stored.
pub fn update(
    store: &Awaits,
    agent: &str,
    conv: &str,
    id: &str,
    change: &Change,
) -> Result<Await> {
    let mut record = mine(store, agent, conv, id)?;
    let now = now_unix();
    if let Some(note) = &change.note {
        record.note = note.trim().to_string();
    }
    if let Some(events) = &change.events {
        for pattern in events {
            check_pattern(pattern)?;
        }
        record.events = events.iter().map(|e| e.trim().to_string()).collect();
    }
    if let Some(when) = &change.when {
        record.when = when.clone();
    }
    if let Some(check) = &change.check {
        record.check = Some(check.trim().to_string()).filter(|c| !c.is_empty());
    }
    if let Some(every) = change.every_seconds {
        record.every = Some(every).filter(|n| *n > 0);
    }
    // A new interval with no deadline behind it would never be looked at: `at` is what the sweep
    // reads, and `every` only ever re-arms it. So an await turned into a poll gets its first look
    // one interval from now, exactly as `register` gives one.
    if let Some(after) = change.after_seconds {
        record.at = Some(now.saturating_add(after.max(1)));
    } else if record.at.is_none() {
        record.at = record.every.map(|every| now.saturating_add(every.max(1)));
    }
    if let Some(expires) = change.expires_in_seconds {
        record.expires_at = Some(now.saturating_add(expires.max(1)));
        record.expiry_wakes = true;
    }
    if record.events.is_empty() && record.at.is_none() {
        return Err(Error::Arguments(format!(
            "that would leave await {id} with nothing to wake on — keep its events, or give it \
             `after_seconds` or `every_seconds`"
        )));
    }
    if !record.when.is_empty() && record.events.is_empty() {
        return Err(Error::Arguments(format!(
            "await {id} would be left filtering payloads with no events to filter — clear `when` \
             too, or keep its events"
        )));
    }
    if !store.claim(&record.id) {
        return Err(Error::Arguments(format!(
            "await {id} fired while this was being changed — it is spent, so register a new one \
             with what you wanted instead"
        )));
    }
    store.save(&record)?;
    Ok(record)
}

/// One of this conversation's own pending awaits, by id.
///
/// The failure is written for whoever will read it as a tool result: a run holding the wrong id is
/// usually holding a spent one, and the ids it *could* have meant are the useful half of the answer.
fn mine(store: &Awaits, agent: &str, conv: &str, id: &str) -> Result<Await> {
    let pending = store.for_conversation(agent, conv);
    if let Some(found) = pending.iter().find(|a| a.id == id) {
        return Ok(found.clone());
    }
    Err(Error::Arguments(if pending.is_empty() {
        format!("no await {id} is pending in this conversation, and nor is any other")
    } else {
        let listed: Vec<String> = pending
            .iter()
            .map(|a| format!("{} ({})", a.id, a.describe()))
            .collect();
        format!(
            "no await {id} is pending in this conversation. Pending here: {}",
            listed.join("; ")
        )
    }))
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
        .filter(|a| a.wants(name, payload))
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

    /// The rule an automatically registered wake stands on. `adi.agents.run.finished` fires for
    /// every run on the machine, so an await that matched the name alone would wake its conversation
    /// on a stranger's ending — and, worse, read it as the one it started.
    #[test]
    fn a_wake_scoped_to_one_run_ignores_every_other_runs_ending() {
        let store = scratch("scoped");
        let mut req = request("the run you started ended");
        req.events = vec!["adi.agents.run.finished".into()];
        req.when = [("run_id".to_string(), "r-42".to_string())]
            .into_iter()
            .collect();
        let saved = register(&store, "watcher", "conv-1", &req).expect("register");

        let name = "adi.agents.run.finished";
        assert!(saved.wants(name, r#"{"agent":"solver","run_id":"r-42","is_error":false}"#));
        assert!(!saved.wants(name, r#"{"agent":"solver","run_id":"r-43","is_error":false}"#));
        assert!(!saved.wants(name, r#"{"agent":"solver"}"#), "a missing field is not a match");
        assert!(!saved.wants("adi.tasks.created", r#"{"run_id":"r-42"}"#), "the name still gates");
        assert!(
            saved.describe().contains("run_id=r-42"),
            "the filter belongs in what the run is told: {}",
            saved.describe()
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A filter that cannot be applied must not read as no filter. A payload the bus published as
    /// something other than an object would otherwise match everything, and the run would take a
    /// stranger's ending for its own.
    #[test]
    fn an_unreadable_payload_matches_nothing() {
        let a = Await {
            id: "w-test".into(),
            agent: "watcher".into(),
            conv: "conv-1".into(),
            note: String::new(),
            events: vec!["adi.agents.run.finished".into()],
            when: [("run_id".to_string(), "r-42".to_string())]
                .into_iter()
                .collect(),
            at: None,
            every: None,
            check: None,
            cwd: String::new(),
            expires_at: None,
            expiry_wakes: false,
            created_at: 0,
        };
        assert!(!a.wants("adi.agents.run.finished", "not json at all"));
        assert!(!a.wants("adi.agents.run.finished", "[]"));
        assert!(!a.wants("adi.agents.run.finished", ""));
    }

    /// A filter with no events to filter is a mistake worth naming: the run believes it is waiting
    /// on one specific thing, and a bare timer would wake it on the first tick regardless.
    #[test]
    fn a_payload_filter_without_events_is_refused() {
        let store = scratch("filter-no-events");
        let req = Request {
            note: "waiting".into(),
            when: [("run_id".to_string(), "r-42".to_string())]
                .into_iter()
                .collect(),
            after_seconds: Some(60),
            ..Request::default()
        };
        let err = register(&store, "watcher", "conv-1", &req).expect_err("refused");
        assert!(err.to_string().contains("`when`"), "{err}");

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// What an action hands back to the run that invoked it: the wake is already registered, and the
    /// two ways out of it are named. A wake nobody asked for that cannot be refused is worse than no
    /// wake at all — it holds one of the few slots the conversation gets.
    #[test]
    fn an_action_registers_a_wake_and_says_how_to_be_rid_of_it() {
        let store = scratch("follow-up");
        let who = Caller {
            agent: "watcher".into(),
            conv: "conv-1".into(),
        };
        let mut req = request("the agent you started ended");
        req.events = vec!["adi.agents.run.finished".into()];
        let note = follow_up(&store, &who, &req);

        let pending = store.for_conversation("watcher", "conv-1");
        assert_eq!(pending.len(), 1, "the wake is registered before the caller reads about it");
        assert!(note.contains(&pending[0].id), "{note}");
        assert!(note.contains("awaits ignore"), "the way out has to be in it: {note}");
        assert!(note.contains("awaits update"), "so does the way to change it: {note}");
        assert!(
            !pending[0].cwd.trim().is_empty(),
            "a check added later has to run somewhere"
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The cap still holds against a wake the run did not ask for, and it is told so rather than
    /// being left believing something will report back.
    #[test]
    fn an_action_that_cannot_register_says_the_work_is_running_anyway() {
        let store = scratch("follow-up-full");
        let who = Caller {
            agent: "watcher".into(),
            conv: "conv-1".into(),
        };
        for i in 0..MAX_PER_CONVERSATION {
            register(&store, "watcher", "conv-1", &request(&format!("wake {i}"))).expect("register");
        }
        let note = follow_up(&store, &who, &request("one too many"));
        assert!(note.starts_with("Nothing will wake you"), "{note}");
        assert!(note.contains("running either way"), "{note}");
        assert_eq!(store.for_conversation("watcher", "conv-1").len(), MAX_PER_CONVERSATION);

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Ignoring is scoped to the conversation that owns the wake. An id travels in plain text and
    /// every await in the store is one directory apart, so without the scope one run could cancel
    /// another's wake and leave it waiting on something that is never coming.
    #[test]
    fn a_wake_is_dropped_by_its_own_conversation_and_by_nobody_else() {
        let store = scratch("ignore");
        let saved = register(&store, "watcher", "conv-1", &request("waiting")).expect("register");

        let err = ignore(&store, "watcher", "conv-2", &saved.id).expect_err("another conversation");
        assert!(err.to_string().contains("no await"), "{err}");
        let err = ignore(&store, "stranger", "conv-1", &saved.id).expect_err("another agent");
        assert!(err.to_string().contains("no await"), "{err}");
        assert_eq!(store.for_conversation("watcher", "conv-1").len(), 1, "still pending");

        let gone = ignore(&store, "watcher", "conv-1", &saved.id).expect("its own");
        assert_eq!(gone.id, saved.id);
        assert!(store.for_conversation("watcher", "conv-1").is_empty());

        let err = ignore(&store, "watcher", "conv-1", &saved.id).expect_err("twice");
        assert!(err.to_string().contains("nor is any other"), "{err}");

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Changing beats replacing: the id survives, so nothing that already named the wake goes stale,
    /// and it never stops watching in the gap a drop-and-re-register would leave.
    #[test]
    fn a_wake_is_changed_in_place_and_keeps_its_id() {
        let store = scratch("update");
        let mut req = request("the old note");
        req.events = vec!["adi.agents.run.finished".into()];
        let saved = register(&store, "watcher", "conv-1", &req).expect("register");
        assert!(saved.at.is_none(), "no deadline yet");

        let changed = update(
            &store,
            "watcher",
            "conv-1",
            &saved.id,
            &Change {
                note: Some("the reason, not the mechanics".into()),
                every_seconds: Some(30),
                check: Some("test -f done".into()),
                ..Change::default()
            },
        )
        .expect("update");

        assert_eq!(changed.id, saved.id, "the id the caller was handed still names it");
        assert_eq!(changed.note, "the reason, not the mechanics");
        assert_eq!(changed.events, saved.events, "what was not named was left alone");
        assert_eq!(changed.every, Some(30));
        assert!(changed.at.is_some(), "a poll with no deadline behind it is never looked at");
        assert_eq!(changed.check.as_deref(), Some("test -f done"));
        assert_eq!(store.for_conversation("watcher", "conv-1"), vec![changed]);

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A change that would leave nothing to wake on is refused, rather than quietly storing a record
    /// the sweep can never reach.
    #[test]
    fn a_change_that_empties_the_wake_condition_is_refused() {
        let store = scratch("update-empty");
        let mut req = request("waiting on events alone");
        req.events = vec!["adi.tasks.created".into()];
        let saved = register(&store, "watcher", "conv-1", &req).expect("register");

        let err = update(
            &store,
            "watcher",
            "conv-1",
            &saved.id,
            &Change {
                events: Some(Vec::new()),
                ..Change::default()
            },
        )
        .expect_err("refused");
        assert!(err.to_string().contains("nothing to wake on"), "{err}");
        assert_eq!(
            store.for_conversation("watcher", "conv-1"),
            vec![saved],
            "the refusal leaves the wake exactly as it was"
        );

        let _ = std::fs::remove_dir_all(store.dir());
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
            when: BTreeMap::new(),
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
            when: BTreeMap::new(),
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
