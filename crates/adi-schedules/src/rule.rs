//! The fact a schedule sweep acts on: one cron rule, bound to a workflow and its own IANA
//! timezone. This crate knows nothing of `on: schedule(cron, tz)` syntax, zod, or the `.ts` file
//! it came from — building a [`ScheduleRule`] out of a workflow file is WF-5's job; this is just
//! what the sweep needs once that's done.

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
    /// `sec min hour day-of-month month day-of-week [year]` — seconds are required,
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
    pub(crate) fn parsed(&self) -> Result<(croner::Cron, jiff::tz::TimeZone)> {
        // Preserve the dialect existing rules were saved with: Sunday is 1 and Saturday 7,
        // and a constrained day-of-month and weekday must both match.
        let schedule = croner::parser::CronParser::builder()
            .seconds(croner::parser::Seconds::Required)
            .year(croner::parser::Year::Optional)
            .alternative_weekdays(true)
            .dom_and_dow(true)
            .build()
            .parse(&normalize_expression(&self.cron)?)
            .map_err(|e| Error::InvalidCron(self.cron.clone(), e.to_string()))?;
        let tz = jiff::tz::TimeZone::get(&self.tz)
            .map_err(|_| Error::UnknownTimezone(self.tz.clone()))?;
        Ok((schedule, tz))
    }
}

/// Preserve the previous parser's bounded fields and point/step shorthand. In particular,
/// years run from 1970 through 2100: Croner's wider year range would change the phase of */N.
fn normalize_expression(expression: &str) -> Result<String> {
    let expression = normalize_names(expression);
    let expanded = match expression.trim() {
        "@yearly" => "0 0 0 1 1 *",
        "@monthly" => "0 0 0 1 * *",
        "@weekly" => "0 0 0 * * 1",
        "@daily" => "0 0 0 * * *",
        "@hourly" => "0 0 * * * *",
        other => other,
    };
    let mut fields: Vec<String> = expanded.split_whitespace().map(str::to_owned).collect();
    if fields.len() == 6 {
        fields.push("*".to_owned());
    }
    if fields.len() != 7 {
        return Err(Error::InvalidCron(
            expression,
            "expected 6 or 7 fields".into(),
        ));
    }
    for (index, field) in fields.iter_mut().enumerate() {
        let max = [59, 59, 23, 31, 12, 7, 2100][index];
        let mut normalized = Vec::new();
        for part in field.split(',') {
            let (base, step) = part
                .split_once('/')
                .map_or((part, None), |(base, step)| (base, Some(step)));
            if index == 6 && base != "*" {
                for year in base.split('-') {
                    if !year
                        .parse::<u32>()
                        .is_ok_and(|year| (1970..=2100).contains(&year))
                    {
                        return Err(Error::InvalidCron(
                            expression,
                            "years must be between 1970 and 2100".into(),
                        ));
                    }
                }
            }
            let base = if index == 6 && base == "*" {
                "1970-2100".to_owned()
            } else if step.is_some() && base.parse::<u32>().is_ok() {
                format!("{base}-{max}")
            } else {
                base.to_owned()
            };
            normalized.push(match step {
                Some(step) => format!("{base}/{step}"),
                None => base,
            });
        }
        *field = normalized.join(",");
    }
    Ok(fields.join(" "))
}

/// The previous parser also accepted full month/day names, plus `Tues` and `Thurs`.
/// Croner expects three-letter names; normalize whole alphabetic tokens so ranges and lists
/// retain those existing spellings without accepting arbitrary name prefixes.
fn normalize_names(expression: &str) -> String {
    expression
        .split_inclusive(|c: char| !c.is_ascii_alphabetic())
        .map(|part| {
            let end = part
                .find(|c: char| !c.is_ascii_alphabetic())
                .unwrap_or(part.len());
            let name = &part[..end];
            let short = match name.to_ascii_lowercase().as_str() {
                "january" => "JAN",
                "february" => "FEB",
                "march" => "MAR",
                "april" => "APR",
                "june" => "JUN",
                "july" => "JUL",
                "august" => "AUG",
                "september" => "SEP",
                "october" => "OCT",
                "november" => "NOV",
                "december" => "DEC",
                "sunday" => "SUN",
                "monday" => "MON",
                "tues" | "tuesday" => "TUE",
                "wednesday" => "WED",
                "thurs" | "thursday" => "THU",
                "friday" => "FRI",
                "saturday" => "SAT",
                _ => name,
            };
            format!("{short}{}", &part[end..])
        })
        .collect()
}
