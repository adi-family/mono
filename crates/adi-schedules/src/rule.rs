//! The fact a schedule sweep acts on: one cron rule, bound to a workflow and its own IANA
//! timezone. This crate knows nothing of `on: schedule(cron, tz)` syntax, zod, or the `.ts` file
//! it came from — building a [`ScheduleRule`] out of a workflow file is WF-5's job; this is just
//! what the sweep needs once that's done.

use std::str::FromStr;

use crate::error::{Error, Result};

/// What happens when more than one slot is due at once — a gap the sweep was down through, not an
/// ordinary tick (exactly one slot due always fires, whatever this is set to; see
/// [`crate::slots::due`]). Spec section D: "Missed slots after downtime fire ONCE; a per-workflow
/// `catchUp: "skip"` option is available." [`CatchUp::Fire`] *is* that default "fire once", named
/// for what it does rather than left as the absence of [`CatchUp::Skip`] — a `catch_up: None`
/// would tempt a match somewhere into reading it as a third state that doesn't exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CatchUp {
    /// Fire once, for the single most recent missed slot; every earlier one in the backlog is
    /// dropped without a run.
    #[default]
    Fire,
    /// Drop the whole backlog silently — no run at all for time the sweep missed.
    Skip,
}

/// One workflow's schedule trigger — spec section E's `on: schedule(cron, tz)`. A workflow file
/// may export several (`on: [schedule(...), schedule(...)]`); each becomes its own `ScheduleRule`,
/// sharing nothing with the others but the `workflow` name they both pour runs for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRule {
    /// The workflow this rule pours runs for — half of the `wf@slot` run key.
    pub workflow: String,
    /// `sec min hour day-of-month month day-of-week [year]` — [`cron::Schedule`]'s own dialect,
    /// not the 5-field unix one. `"0 0 9 * * Mon-Fri"` is "9am every weekday".
    pub cron: String,
    /// An IANA zone name (`"America/New_York"`), never a bare offset — the whole point is that it
    /// carries its own DST transitions, which an offset alone can't.
    pub tz: String,
    pub catch_up: CatchUp,
}

impl ScheduleRule {
    /// A rule with the default catch-up behaviour ([`CatchUp::Fire`]) — use
    /// [`skip_catch_up`](Self::skip_catch_up) to opt out.
    pub fn new(
        workflow: impl Into<String>,
        cron: impl Into<String>,
        tz: impl Into<String>,
    ) -> Self {
        Self {
            workflow: workflow.into(),
            cron: cron.into(),
            tz: tz.into(),
            catch_up: CatchUp::default(),
        }
    }

    #[must_use]
    pub fn skip_catch_up(mut self) -> Self {
        self.catch_up = CatchUp::Skip;
        self
    }

    /// Parse and validate both halves at once — a rule with a bad cron expression or an unknown
    /// zone name is refused here, every time it is resolved, rather than scheduled once and then
    /// silently never firing.
    pub(crate) fn parsed(&self) -> Result<(cron::Schedule, chrono_tz::Tz)> {
        let schedule = cron::Schedule::from_str(&self.cron)
            .map_err(|e| Error::InvalidCron(self.cron.clone(), e.to_string()))?;
        let tz = self
            .tz
            .parse::<chrono_tz::Tz>()
            .map_err(|_| Error::UnknownTimezone(self.tz.clone()))?;
        Ok((schedule, tz))
    }
}
