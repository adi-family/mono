//! Pure due-slot computation — no I/O, no store, nothing but a rule, a cursor, and a clock.
//! [`due`] is the one function the store ([`crate::store`]) and every test here go through.

use jiff::Timestamp;

use crate::error::Result;
use crate::rule::{CatchUp, ScheduleRule};

/// What one evaluation resolved for a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    /// The one slot to actually pour a run for, if any. Never more than one, however many slots
    /// were missed — see the module docs on why.
    pub fire: Option<Timestamp>,
    /// What the workflow's cursor should become after this evaluation, fire or not. `None` only
    /// when nothing was due at all, which leaves the cursor exactly where it was.
    pub advance_to: Option<Timestamp>,
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
/// Jiff), then converted back to UTC for comparison and for the returned slots — so a
/// "9am daily" rule keeps meaning 9am local through a DST transition, landing on two different UTC
/// instants either side of it, rather than drifting by an hour. Croner generates civil-time
/// candidates and Jiff resolves each in that zone: a spring-forward gap is skipped outright,
/// and a fall-back repeat yields both occurrences, earlier then later. The next search starts
/// one absolute second after the last yielded instant, preserving the previous parser's order
/// even inside a repeated hour. Civil iteration also avoids Croner's fixed-job gap catch-up.
///
/// # Errors
///
/// [`crate::Error::InvalidCron`] or [`crate::Error::UnknownTimezone`] if `rule` itself doesn't
/// parse.
pub fn due(rule: &ScheduleRule, last: Option<Timestamp>, now: Timestamp) -> Result<Resolved> {
    let (schedule, tz) = rule.parsed()?;
    let Some(anchor) = last else {
        return Ok(Resolved {
            fire: None,
            advance_to: Some(now),
        });
    };

    let mut latest = None;
    let mut multiple = false;
    let mut previous = anchor;
    let mut pending_later = None;
    loop {
        let slot = if let Some(later) = pending_later.take() {
            later
        } else {
            // cron 0.17 advanced the absolute clock by one second, then used its local fields
            // (without subseconds). This matters at the exact instant the clock falls back.
            let Some(start) = previous
                .as_second()
                .checked_add(1)
                .and_then(|second| Timestamp::from_second(second).ok())
            else {
                break;
            };
            let Some(ambiguous) = schedule
                .iter_from(tz.to_datetime(start), croner::Direction::Forward)
                .map(|candidate| tz.to_ambiguous_timestamp(candidate))
                .find(|candidate| {
                    !matches!(candidate.offset(), jiff::tz::AmbiguousOffset::Gap { .. })
                })
            else {
                break;
            };
            if matches!(ambiguous.offset(), jiff::tz::AmbiguousOffset::Fold { .. }) {
                pending_later =
                    Some(ambiguous.clone().later().map_err(|e| {
                        crate::Error::InvalidCron(rule.cron.clone(), e.to_string())
                    })?);
            }
            ambiguous
                .earlier()
                .map_err(|e| crate::Error::InvalidCron(rule.cron.clone(), e.to_string()))?
        };
        if slot > now {
            break;
        }
        previous = slot;
        multiple |= latest.is_some();
        latest = Some(slot);
    }

    let Some(latest) = latest else {
        return Ok(Resolved {
            fire: None,
            advance_to: None,
        });
    };
    let fire = if multiple && rule.catch_up == CatchUp::Skip {
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

    /// Every minute, at second 0 — easy to reason about in test fixtures without pulling in real
    /// wall-clock dates.
    fn every_minute() -> ScheduleRule {
        ScheduleRule::new("wf", "0 * * * * *", "UTC")
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
            .parse()
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
    fn a_time_in_the_spring_forward_gap_is_skipped() {
        let rule = ScheduleRule::new("wf", "0 30 2 * * *", "America/New_York");
        let resolved = due(
            &rule,
            Some(utc(2026, 3, 8, 5, 0, 0)),
            utc(2026, 3, 8, 12, 0, 0),
        )
        .expect("due");
        assert_eq!(resolved.fire, None, "02:30 never occurs on this day");
        assert_eq!(resolved.advance_to, None);
    }

    #[test]
    fn individual_ticks_keep_the_existing_cursor_behavior_in_a_repeated_hour() {
        let rule = ScheduleRule::new("wf", "0 30 1 * * *", "America/New_York");
        let first = due(
            &rule,
            Some(utc(2026, 11, 1, 4, 0, 0)),
            utc(2026, 11, 1, 5, 30, 0),
        )
        .expect("due");
        assert_eq!(first.fire, Some(utc(2026, 11, 1, 5, 30, 0)));
        let repeat = due(&rule, first.advance_to, utc(2026, 11, 1, 6, 30, 0)).expect("due");
        assert_eq!(repeat.fire, None);
    }

    #[test]
    fn a_fall_back_backlog_keeps_the_existing_earlier_then_later_iteration_order() {
        let rule = ScheduleRule::new("wf", "0 * * * * *", "America/New_York");
        let resolved = due(
            &rule,
            Some(utc(2026, 11, 1, 5, 5, 0)),
            utc(2026, 11, 1, 6, 10, 0),
        )
        .expect("due");
        // The old iterator yields 01:06 EDT, 01:06 EST, 01:07 EDT, ... . It stops when
        // 01:11 EST is in the future, leaving 01:11 EDT as the final consumed occurrence.
        assert_eq!(resolved.fire, Some(utc(2026, 11, 1, 5, 11, 0)));
    }

    #[test]
    fn a_backlog_contains_both_occurrences_of_a_repeated_fixed_time() {
        let rule = ScheduleRule::new("wf", "0 30 1 * * *", "America/New_York");
        let last = Some(utc(2026, 11, 1, 4, 0, 0));
        let now = utc(2026, 11, 1, 7, 0, 0);
        assert_eq!(
            due(&rule, last, now).unwrap().fire,
            Some(utc(2026, 11, 1, 6, 30, 0))
        );
        let skipped = due(&rule.skip_catch_up(), last, now).unwrap();
        assert_eq!(skipped.fire, None, "both occurrences constitute a backlog");
        assert_eq!(skipped.advance_to, Some(utc(2026, 11, 1, 6, 30, 0)));
    }

    #[test]
    fn a_cursor_in_the_second_repeated_hour_preserves_the_previous_iteration_order() {
        let rule = ScheduleRule::new("wf", "0 * * * * *", "America/New_York");
        let resolved = due(
            &rule,
            Some(utc(2026, 11, 1, 6, 5, 0)),
            utc(2026, 11, 1, 6, 40, 0),
        )
        .unwrap();
        assert_eq!(resolved.fire, Some(utc(2026, 11, 1, 5, 41, 0)));
    }

    #[test]
    fn the_fall_back_boundary_advances_one_absolute_second_before_reading_local_fields() {
        let rule = ScheduleRule::new("wf", "0 * * * * *", "America/New_York");
        let resolved = due(
            &rule,
            Some(utc(2026, 11, 1, 5, 59, 59)),
            utc(2026, 11, 1, 6, 0, 0),
        )
        .unwrap();
        assert_eq!(resolved.fire, Some(utc(2026, 11, 1, 5, 1, 0)));
    }

    #[test]
    fn point_steps_and_year_step_phase_keep_the_previous_field_bounds() {
        for expression in ["0 5/10 * * * *", "0 5-59/10 * * * *"] {
            let rule = ScheduleRule::new("wf", expression, "UTC");
            assert_eq!(
                due(
                    &rule,
                    Some(utc(2026, 1, 1, 12, 4, 0)),
                    utc(2026, 1, 1, 12, 26, 0)
                )
                .unwrap()
                .fire,
                Some(utc(2026, 1, 1, 12, 25, 0))
            );
        }
        for years in ["*/2", "2026/2", "2026-2100/2"] {
            let rule = ScheduleRule::new("wf", format!("0 0 0 1 1 * {years}"), "UTC");
            assert_eq!(
                due(
                    &rule,
                    Some(utc(2025, 12, 31, 0, 0, 0)),
                    utc(2028, 1, 2, 0, 0, 0)
                )
                .unwrap()
                .fire,
                Some(utc(2028, 1, 1, 0, 0, 0)),
                "{years}"
            );
            assert_eq!(
                due(
                    &rule,
                    Some(utc(2100, 1, 1, 0, 0, 0)),
                    utc(2104, 1, 2, 0, 0, 0)
                )
                .unwrap()
                .fire,
                None
            );
        }
        let unbounded = ScheduleRule::new("wf", "0 0 0 1 1 *", "UTC");
        assert_eq!(
            due(
                &unbounded,
                Some(utc(2100, 1, 1, 0, 0, 0)),
                utc(2101, 1, 2, 0, 0, 0)
            )
            .unwrap()
            .fire,
            None
        );
    }

    #[test]
    fn numeric_weekdays_years_and_day_intersection_keep_their_existing_meaning() {
        // Sunday is 1, and both the date and weekday must match. November 1, 2026 is Sunday.
        let rule = ScheduleRule::new("wf", "0 0 9 1 * 1 2026", "UTC");
        let resolved = due(
            &rule,
            Some(utc(2026, 10, 1, 0, 0, 0)),
            utc(2026, 11, 2, 0, 0, 0),
        )
        .expect("due");
        assert_eq!(resolved.fire, Some(utc(2026, 11, 1, 9, 0, 0)));
        let finished = due(&rule, resolved.advance_to, utc(2027, 11, 2, 0, 0, 0)).expect("due");
        assert_eq!(finished.fire, None, "the explicit year is honored");
    }

    #[test]
    fn the_weekly_alias_still_means_sunday_midnight() {
        let rule = ScheduleRule::new("wf", "@weekly", "UTC");
        let resolved = due(
            &rule,
            Some(utc(2026, 10, 31, 0, 0, 0)),
            utc(2026, 11, 1, 12, 0, 0),
        )
        .expect("due");
        assert_eq!(resolved.fire, Some(utc(2026, 11, 1, 0, 0, 0)));
    }

    #[test]
    fn named_months_weekday_ranges_and_lists_keep_their_existing_meaning() {
        for expression in [
            "0 0 9 * NOV MON-WED",
            "0 0 9 * November Monday-Wednesday",
            "0 0 9 * November MON,Tues,Wednesday",
        ] {
            let rule = ScheduleRule::new("wf", expression, "UTC");
            let resolved = due(
                &rule,
                Some(utc(2026, 11, 1, 0, 0, 0)),
                utc(2026, 11, 5, 12, 0, 0),
            )
            .expect("due");
            assert_eq!(
                resolved.fire,
                Some(utc(2026, 11, 4, 9, 0, 0)),
                "{expression}"
            );
        }
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
