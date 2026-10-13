//! The WORKFLOWS schedule sweep — spec section D of ADI-MONO-141 (`schedule(cron, tz)` triggers),
//! built standalone and ahead of WF-5's engine on purpose: everything here is testable without a
//! single `.ts` workflow file existing yet, and the engine, once it does, is this crate's only
//! caller.
//!
//! # What this crate owns, and what it leaves to WF-5
//!
//! A [`ScheduleRule`] is a bare fact — a workflow id, a cron expression, an IANA zone, a
//! catch-up policy — with no idea where it came from. Reading a workflow file's `on:
//! schedule(cron, tz)` export into one of these, and deciding what a pour actually *does* (create
//! the run row, the root task, emit `adi.workflows.run.started`), both belong to the engine. This
//! crate is the part in between: given a rule and "what slot did we last resolve to", which slot
//! (if any) is due *now*, and making sure that slot's `wf@slot` run key is claimed once, ever, even
//! if the engine's own tick calls in twice or two processes race it.
//!
//! # The three pieces
//!
//! * [`slots::due`] — pure computation, no I/O. Given a rule, the last resolved slot, and now, what
//!   fires and what the cursor becomes. [`CatchUp`] is read here: more than one slot due at once is
//!   a backlog (the sweep was down, not just late), which fires once under [`CatchUp::Fire`] or not
//!   at all under [`CatchUp::Skip`] — exactly one slot due always fires either way, since that is
//!   the schedule's ordinary job, not a catch-up.
//! * [`ScheduleStore`] — `schedules.db`, under its own module dir. Wraps [`slots::due`] in one
//!   write transaction per rule, so "the cursor advanced" and "the run key was claimed" can never
//!   disagree about what happened.
//! * [`tick`] — the sweep itself, and the seam WF-5 is meant to call through: implement
//!   [`ScheduleSource`] (list your workflows' schedules) and [`RunPourer`] (what a due slot does),
//!   and `tick` is a drop-in for whatever tick loop the engine runs on (the same loop as
//!   `adi-agents`' own dispatch/goals/awaits ticks, most likely, though this crate has no opinion
//!   on where it's wired in — that choice belongs to whoever owns the loop).
//!
//! # Timezones and DST
//!
//! Every rule names an IANA zone (`tz`), resolved by `chrono-tz`'s compiled copy of the database —
//! never a bare UTC offset, which is the one representation that can't carry a DST transition.
//! Slots are computed, compared, and returned in UTC throughout; only the *evaluation* of "does
//! 9am mean 13:00 or 14:00 today" happens in the rule's own zone. See `slots`' module tests for the
//! two transitions worked through explicitly.

mod error;
mod rule;
mod slots;
mod store;
mod sweep;

pub use error::{Error, Result};
pub use rule::{CatchUp, ScheduleRule};
pub use slots::{Resolved, due};
pub use store::{Due, ScheduleStore};
pub use sweep::{RunPourer, ScheduleSource, tick};
