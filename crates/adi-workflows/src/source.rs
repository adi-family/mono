//! Reading every workflow file's `on: schedule(cron, tz)` triggers into [`ScheduleRule`]s —
//! [`adi_schedules::ScheduleSource`]'s one method, and exactly the piece of spec section E's
//! discovery `adi-schedules` itself deliberately knows nothing about.

use adi_config::Config;
use adi_schedules::ScheduleRule;

use crate::cron5::normalize_five_field;
use crate::describe::{self, TriggerDef};
use crate::discover;
use crate::store::WorkflowStore;

/// Adapts this crate's workflow-file discovery into what `adi_schedules::tick` sweeps.
#[derive(Debug, Clone)]
pub struct WorkflowScheduleSource {
    config: Config,
    store: WorkflowStore,
}

impl WorkflowScheduleSource {
    #[must_use]
    pub fn new(config: Config) -> Self {
        let store = WorkflowStore::with_config(config.clone());
        Self { config, store }
    }
}

impl adi_schedules::ScheduleSource for WorkflowScheduleSource {
    /// Every schedule trigger of every **enabled** workflow file, normalized to `adi_schedules`'
    /// own cron dialect — a disabled workflow (the default for one this store has never been told
    /// about; see [`WorkflowStore::is_enabled`]) contributes nothing here, same as one with no
    /// `schedule` trigger at all. A file that fails to describe, or a rule whose cron isn't valid
    /// 5-field unix cron, is logged and skipped — one broken workflow must never stop every other
    /// schedule in the same sweep (mirrors `adi_schedules::tick`'s own per-rule resilience).
    fn schedules(&self) -> Vec<ScheduleRule> {
        let mut rules = Vec::new();
        for file in discover::workflow_files(&self.config) {
            if !self.store.is_enabled(&file.id).unwrap_or(false) {
                continue;
            }
            let shape = match describe::describe(&self.config, &file.path) {
                Ok(shape) => shape,
                Err(e) => {
                    tracing::warn!(
                        workflow = %file.id, path = %file.path.display(), error = %e,
                        "couldn't describe a workflow file; it has no schedules this tick"
                    );
                    continue;
                }
            };
            for trigger in shape.on {
                let TriggerDef::Schedule { cron, tz, catch_up } = trigger else {
                    continue;
                };
                let normalized = match normalize_five_field(&cron) {
                    Ok(normalized) => normalized,
                    Err(e) => {
                        tracing::warn!(
                            workflow = %file.id, cron = %cron, error = %e,
                            "a workflow's schedule cron is invalid; skipping just this rule"
                        );
                        continue;
                    }
                };
                let mut rule = ScheduleRule::new(file.id.clone(), normalized, tz);
                if catch_up.as_deref() == Some("skip") {
                    rule = rule.skip_catch_up();
                }
                rules.push(rule);
            }
        }
        rules
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_schedules::ScheduleSource;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-workflows-source-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    fn write_workflow(config: &Config, name: &str, body: &str) {
        let dir = config.module("workflows").dir().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    /// Every test here is about what a trigger *looks like* once a workflow is already trusted
    /// with its own schedule — `enabled`'s own default (disabled) is covered by
    /// `a_disabled_workflows_schedule_is_not_swept` below, not re-asserted in every other test.
    fn enable(config: &Config, workflow: &str) {
        WorkflowStore::with_config(config.clone())
            .set_enabled(workflow, true)
            .expect("enable");
    }

    #[test]
    fn a_disabled_workflows_schedule_is_not_swept() {
        let config = scratch("disabled-default");
        describe::bootstrap(&config).expect("bootstrap");
        write_workflow(
            &config,
            "weekly.ts",
            "import { schedule } from \"@adi/workflows\";\n\
             export const on = schedule(\"0 5 * * MON\", \"UTC\");\n",
        );
        assert!(
            WorkflowScheduleSource::new(config.clone()).schedules().is_empty(),
            "a never-enabled workflow contributes no schedule rule"
        );
        enable(&config, "weekly");
        assert_eq!(WorkflowScheduleSource::new(config).schedules().len(), 1);
    }

    #[test]
    fn a_schedule_trigger_is_normalized_into_a_six_field_rule() {
        let config = scratch("normalize");
        describe::bootstrap(&config).expect("bootstrap");
        write_workflow(
            &config,
            "weekly.ts",
            "import { schedule } from \"@adi/workflows\";\n\
             export const on = schedule(\"0 5 * * MON\", \"America/New_York\");\n",
        );
        enable(&config, "weekly");

        let rules = WorkflowScheduleSource::new(config).schedules();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].workflow, "weekly");
        assert_eq!(rules[0].cron, "0 0 5 * * MON");
        assert_eq!(rules[0].tz, "America/New_York");
    }

    #[test]
    fn catch_up_skip_is_carried_through() {
        let config = scratch("skip");
        describe::bootstrap(&config).expect("bootstrap");
        write_workflow(
            &config,
            "weekly.ts",
            "import { schedule } from \"@adi/workflows\";\n\
             export const on = schedule(\"0 5 * * MON\", \"UTC\", { catchUp: \"skip\" });\n",
        );
        enable(&config, "weekly");

        let rules = WorkflowScheduleSource::new(config).schedules();
        assert_eq!(rules[0].catch_up, adi_schedules::CatchUp::Skip);
    }

    #[test]
    fn an_event_only_workflow_contributes_no_schedule_rules() {
        let config = scratch("event-only");
        describe::bootstrap(&config).expect("bootstrap");
        write_workflow(
            &config,
            "on-task.ts",
            "import { event } from \"@adi/workflows\";\n\
             export const on = event(\"adi.tasks.created\");\n",
        );
        assert!(WorkflowScheduleSource::new(config).schedules().is_empty());
    }

    #[test]
    fn a_bad_cron_skips_only_that_rule() {
        let config = scratch("bad-cron");
        describe::bootstrap(&config).expect("bootstrap");
        write_workflow(
            &config,
            "broken.ts",
            "import { schedule } from \"@adi/workflows\";\n\
             export const on = schedule(\"not a cron\", \"UTC\");\n",
        );
        write_workflow(
            &config,
            "fine.ts",
            "import { schedule } from \"@adi/workflows\";\n\
             export const on = schedule(\"0 5 * * MON\", \"UTC\");\n",
        );
        enable(&config, "broken");
        enable(&config, "fine");
        let rules = WorkflowScheduleSource::new(config).schedules();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].workflow, "fine");
    }
}
