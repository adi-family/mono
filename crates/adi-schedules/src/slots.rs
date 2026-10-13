//! Pure due-slot computation — no I/O, no store, nothing but a rule, a cursor, and a clock.
//! [`due`] is the one function the store ([`crate::store`]) and every test here go through.

use chrono::{DateTime, Utc};

use crate::error::Result;
use crate::rule::{CatchUp, ScheduleRule};

/// What one evaluation resolved for a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    /// The one slot to actually pour a run for, if any. Never more than one, however many slots
    /// were missed — see the module docs on why.
    pub fire: Option<DateTime<Utc>>,
    /// What the workflow's cursor should become after this evaluation, fire or not. `None` only
    /// when nothing was due at all, which leaves the cursor exactly where it was.
    pub advance_to: Option<DateTime<Utc>>,
}

/// Every slot `rule`'s cron owes since `last`, collapsed to at most one fire.
///
/// # Catch-up
///
/// More than one occurrence between `last` and `now` is a backlog — the sweep was down for a
/// while, not just late by a tick — and per spec section D, that fires **once**, for the single
/// most recent missed slot, under the default [`CatchUp::Fire`]; [`CatchUp::Skip`] fires none of
/// it. Exactly one occurrence always fires regardless of `catch_up`, because that is the schedule
/// doing its ordinary job, not catching anything up. Either way `advance_to` becomes the latest
/// occurrence found, so a backlog this call skipped is never recomputed and re-skipped on the
/// next — it is skipped exactly once, here.
///
/// # A schedule with nothing poured yet
///
/// `last == None` anchors the cursor at `now` and fires nothing: a workflow enabled this moment
/// must not immediately backfire for whatever slot its cron already passed earlier today, before
/// the schedule existed. The first real fire is the next occurrence from here on.
///
/// # Timezone and DST
///
/// Resolution happens entirely in `rule`'s own IANA zone (`rule.tz`, parsed and handled by
/// `chrono-tz`), then converted back to UTC for comparison and for the returned slots — so a
/// "9am daily" rule keeps meaning 9am local through a DST transition, landing on two different UTC
/// instants either side of it, rather than drifting by an hour. The underlying `cron` crate
/// resolves each wall-clock candidate as it goes: a spring-forward gap (no such local time) is
/// skipped outright, and a fall-back repeat (the same local time occurring twice) resolves to its
/// earlier occurrence — both deterministic, neither this module's own logic to get right.
///
/// # Errors
///
/// [`crate::Error::InvalidCron`] or [`crate::Error::UnknownTimezone`] if `rule` itself doesn't
/// parse.
pub fn due(
    rule: &ScheduleRule,
    last: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Resolved> {
    let (schedule, tz) = rule.parsed()?;
    let Some(anchor) = last else {
        return Ok(Resolved {
            fire: None,
            advance_to: Some(now),
        });
    };

    let anchor_local = anchor.with_timezone(&tz);
    let now_local = now.with_timezone(&tz);
    let occurrences: Vec<DateTime<Utc>> = schedule
        .after(&anchor_local)
        .take_while(|slot| *slot <= now_local)
        .map(|slot| slot.with_timezone(&Utc))
        .collect();

    let Some(&latest) = occurrences.last() else {
        return Ok(Resolved {
            fire: None,
            advance_to: None,
        });
    };
    let fire = if occurrences.len() > 1 && rule.catch_up == CatchUp::Skip {
        None
    } else {
        Some(latest)
    };
    Ok(Resolved {
        fire,
        advance_to: Some(latest),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Every minute, at second 0 — easy to reason about in test fixtures without pulling in real
    /// wall-clock dates.
    fn every_minute() -> ScheduleRule {
        ScheduleRule::new("wf", "0 * * * * *", "UTC")
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("valid instant")
    }

    #[test]
    fn nothing_poured_yet_anchors_at_now_and_fires_nothing() {
        let now = utc(2026, 1, 1, 12, 0, 30);
        let resolved = due(&every_minute(), None, now).expect("due");
        assert_eq!(resolved.fire, None);
        assert_eq!(resolved.advance_to, Some(now));
    }

    #[test]
    fn exactly_one_missed_slot_fires_even_under_skip() {
        let last = utc(2026, 1, 1, 12, 0, 0);
        let now = utc(2026, 1, 1, 12, 1, 30);
        let rule = every_minute().skip_catch_up();
        let resolved = due(&rule, Some(last), now).expect("due");
        assert_eq!(resolved.fire, Some(utc(2026, 1, 1, 12, 1, 0)));
        assert_eq!(resolved.advance_to, resolved.fire);
    }

    #[test]
    fn a_backlog_fires_only_the_most_recent_slot_by_default() {
        let last = utc(2026, 1, 1, 12, 0, 0);
        // Five minutes passed with no tick in between — five occurrences missed.
        let now = utc(2026, 1, 1, 12, 5, 30);
        let resolved = due(&every_minute(), Some(last), now).expect("due");
        assert_eq!(
            resolved.fire,
            Some(utc(2026, 1, 1, 12, 5, 0)),
            "only the latest, not all five"
        );
        assert_eq!(
            resolved.advance_to, resolved.fire,
            "the cursor skips the whole backlog at once"
        );
    }

    #[test]
    fn catch_up_skip_fires_nothing_for_a_backlog_but_still_advances() {
        let last = utc(2026, 1, 1, 12, 0, 0);
        let now = utc(2026, 1, 1, 12, 5, 30);
        let rule = every_minute().skip_catch_up();
        let resolved = due(&rule, Some(last), now).expect("due");
        assert_eq!(resolved.fire, None, "the whole backlog is dropped");
        assert_eq!(
            resolved.advance_to,
            Some(utc(2026, 1, 1, 12, 5, 0)),
            "but the cursor still skips past it, so it is never recomputed"
        );
    }

    #[test]
    fn nothing_due_yet_leaves_the_cursor_untouched() {
        let last = utc(2026, 1, 1, 12, 0, 0);
        let now = utc(2026, 1, 1, 12, 0, 30);
        let resolved = due(&every_minute(), Some(last), now).expect("due");
        assert_eq!(resolved.fire, None);
        assert_eq!(resolved.advance_to, None);
    }

    #[test]
    fn a_daily_9am_rule_keeps_meaning_9am_local_across_spring_forward() {
        // US DST 2026: clocks spring forward from EST (-05:00) to EDT (-04:00) at
        // 2026-03-08 02:00 local, in America/New_York. A 9am-local rule should read 14:00 UTC the
        // day before the transition and 13:00 UTC the day of it — the same wall-clock hour, a
        // different offset, never drifting into some other local time.
        let rule = ScheduleRule::new("wf", "0 0 9 * * *", "America/New_York");

        let before = due(
            &rule,
            Some(utc(2026, 3, 6, 20, 0, 0)),
            utc(2026, 3, 7, 20, 0, 0),
        )
        .expect("due");
        assert_eq!(
            before.fire,
            Some(utc(2026, 3, 7, 14, 0, 0)),
            "9am EST on Mar 7 is 14:00 UTC"
        );

        let after = due(&rule, before.advance_to, utc(2026, 3, 8, 20, 0, 0)).expect("due");
        assert_eq!(
            after.fire,
            Some(utc(2026, 3, 8, 13, 0, 0)),
            "9am EDT on Mar 8 is 13:00 UTC"
        );
    }

    #[test]
    fn a_daily_9am_rule_keeps_meaning_9am_local_across_the_fall_back() {
        // 2026-11-01: America/New_York falls back from EDT (-04:00) to EST (-05:00) at 02:00
        // local. Same check, the other direction: 13:00 UTC the day before, 14:00 UTC the day of.
        let rule = ScheduleRule::new("wf", "0 0 9 * * *", "America/New_York");

        let before = due(
            &rule,
            Some(utc(2026, 10, 30, 20, 0, 0)),
            utc(2026, 10, 31, 20, 0, 0),
        )
        .expect("due");
        assert_eq!(
            before.fire,
            Some(utc(2026, 10, 31, 13, 0, 0)),
            "9am EDT on Oct 31 is 13:00 UTC"
        );

        let after = due(&rule, before.advance_to, utc(2026, 11, 1, 20, 0, 0)).expect("due");
        assert_eq!(
            after.fire,
            Some(utc(2026, 11, 1, 14, 0, 0)),
            "9am EST on Nov 1 is 14:00 UTC"
        );
    }

    #[test]
    fn an_invalid_cron_expression_is_refused() {
        let rule = ScheduleRule::new("wf", "not a cron expression", "UTC");
        assert!(matches!(
            due(&rule, None, utc(2026, 1, 1, 0, 0, 0)),
            Err(crate::Error::InvalidCron(_, _))
        ));
    }

    #[test]
    fn an_unknown_timezone_is_refused() {
        let rule = ScheduleRule::new("wf", "0 * * * * *", "Mars/Olympus_Mons");
        assert!(matches!(
            due(&rule, None, utc(2026, 1, 1, 0, 0, 0)),
            Err(crate::Error::UnknownTimezone(_))
        ));
    }
}
