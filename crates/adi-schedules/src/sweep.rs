//! The sweep itself: the one function the app (or, in this crate's own tests, a fixture) calls
//! once a tick, and the two traits WF-5's workflow engine implements to be that caller for real —
//! see the crate docs for why the seam sits here rather than this crate reading `.ts` files or
//! owning a run row of its own.

use chrono::{DateTime, Utc};

use crate::rule::ScheduleRule;
use crate::store::{Due, ScheduleStore};

/// Every workflow with at least one schedule trigger, as of this tick. Supplied by whatever reads
/// the workflow files (`on: schedule(cron, tz)`, spec section E) — this crate never parses one
/// itself.
pub trait ScheduleSource {
    fn schedules(&self) -> Vec<ScheduleRule>;
}

/// What happens once a slot is due and already uniquely claimed (see [`ScheduleStore::resolve`]):
/// build whatever `due.run_key` demands — for WF-5, the run row and its root task. `pour` is never
/// called twice for the same `run_key`; it does not need its own dedupe.
pub trait RunPourer {
    fn pour(&self, due: &Due);
}

/// Sweep every schedule `source` reports, resolving each against `store` and calling `pourer` for
/// whatever is newly due. Returns what fired, in the order `source` listed its schedules — the
/// same shape `adi-agents`' own dispatch and goal ticks use for their own "what happened"
/// reports: only what actually fired is worth reporting, not every rule a tick found nothing to
/// do for.
///
/// A rule that fails to resolve (a cron expression or timezone that doesn't parse) is logged and
/// skipped, not propagated — one bad workflow file must never stop every other schedule in the
/// same tick.
pub fn tick(
    store: &ScheduleStore,
    source: &dyn ScheduleSource,
    pourer: &dyn RunPourer,
    now: DateTime<Utc>,
) -> Vec<Due> {
    let mut fired = Vec::new();
    for rule in source.schedules() {
        match store.resolve(&rule, now) {
            Ok(Some(due)) => {
                pourer.pour(&due);
                fired.push(due);
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    workflow = %rule.workflow, cron = %rule.cron, tz = %rule.tz, error = %e,
                    "schedule sweep could not resolve this rule"
                );
            }
        }
    }
    fired
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_config::Config;
    use chrono::TimeZone;
    use std::cell::RefCell;

    fn scratch(tag: &str) -> ScheduleStore {
        let root = std::env::temp_dir().join(format!(
            "adi-schedules-sweep-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        ScheduleStore::with_config(Config::with_root(root))
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("valid instant")
    }

    struct FixedSchedules(Vec<ScheduleRule>);
    impl ScheduleSource for FixedSchedules {
        fn schedules(&self) -> Vec<ScheduleRule> {
            self.0.clone()
        }
    }

    #[derive(Default)]
    struct RecordingPourer(RefCell<Vec<Due>>);
    impl RunPourer for RecordingPourer {
        fn pour(&self, due: &Due) {
            self.0.borrow_mut().push(due.clone());
        }
    }

    #[test]
    fn a_tick_with_nothing_due_pours_nothing() {
        let store = scratch("nothing-due");
        let source = FixedSchedules(vec![ScheduleRule::new("wf", "0 * * * * *", "UTC")]);
        let pourer = RecordingPourer::default();

        // First tick ever: establishes the cursor, never fires.
        assert!(tick(&store, &source, &pourer, utc(2026, 1, 1, 12, 0, 30)).is_empty());
        assert!(pourer.0.borrow().is_empty());
    }

    #[test]
    fn a_due_slot_is_poured_exactly_once_across_several_ticks() {
        let store = scratch("poured-once");
        let source = FixedSchedules(vec![ScheduleRule::new("wf", "0 * * * * *", "UTC")]);
        let pourer = RecordingPourer::default();

        tick(&store, &source, &pourer, utc(2026, 1, 1, 12, 0, 0));
        let fired = tick(&store, &source, &pourer, utc(2026, 1, 1, 12, 1, 0));
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].run_key, "wf@2026-01-01T12:01:00Z");

        // The same instant swept again (the app's own tick loop runs every second, far more often
        // than this cron fires) pours nothing more.
        let again = tick(&store, &source, &pourer, utc(2026, 1, 1, 12, 1, 0));
        assert!(again.is_empty());
        assert_eq!(
            pourer.0.borrow().len(),
            1,
            "pour is called exactly once for this slot, ever"
        );
    }

    #[test]
    fn two_workflows_are_swept_independently_in_one_tick() {
        let store = scratch("two-workflows");
        let source = FixedSchedules(vec![
            ScheduleRule::new("a", "0 * * * * *", "UTC"),
            ScheduleRule::new("b", "0 * * * * *", "UTC"),
        ]);
        let pourer = RecordingPourer::default();
        tick(&store, &source, &pourer, utc(2026, 1, 1, 12, 0, 0));
        let fired = tick(&store, &source, &pourer, utc(2026, 1, 1, 12, 1, 0));
        let workflows: Vec<&str> = fired.iter().map(|d| d.workflow.as_str()).collect();
        assert_eq!(workflows, vec!["a", "b"]);
    }

    #[test]
    fn a_rule_that_fails_to_parse_is_skipped_without_stopping_the_rest() {
        let store = scratch("bad-rule-skipped");
        let source = FixedSchedules(vec![
            ScheduleRule::new("broken", "nonsense", "UTC"),
            ScheduleRule::new("fine", "0 * * * * *", "UTC"),
        ]);
        let pourer = RecordingPourer::default();
        tick(&store, &source, &pourer, utc(2026, 1, 1, 12, 0, 0));
        let fired = tick(&store, &source, &pourer, utc(2026, 1, 1, 12, 1, 0));
        assert_eq!(
            fired.len(),
            1,
            "the broken rule never fires, but the working one still does"
        );
        assert_eq!(fired[0].workflow, "fine");
    }
}
