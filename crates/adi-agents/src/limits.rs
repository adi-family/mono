//! How many agent runs may be live at once — overall, and per project.
//!
//! A run is a detached process (or a pty session), and nothing about a *definition* bounds how many
//! of them exist: a trigger firing on every event, a chat queue draining, and a human clicking Run
//! all launch through the same door. These are the throttles on that door, stored at
//! `sessions/settings.toml`, next to the runs they count:
//!
//! ```toml
//! max_concurrent_runs = 3   # the ceiling, across every agent
//!
//! [projects]
//! bugbounty = 2             # …and at most 2 of those may be this project's
//! ```
//!
//! The global number is a ceiling, not a default: a project limit narrows it further and can never
//! lift it. A project with no entry is bounded only by the global one.
//!
//! Both caps bind every launch the platform makes on its own (a queued turn starting, a trigger
//! firing `adi-agents run`). A human is never *stopped* by them, only told: a refused launch says
//! which cap is full, and asking again with `force` runs anyway — see [`Agents::force_run_in`].
//!
//! [`Agents::force_run_in`]: crate::Agents::force_run_in

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use adi_config::Module;

use crate::error::Result;

/// How many runs may be live before an unforced launch is refused. Deliberately small: these are
/// full agent processes, each with its own model calls and tool children.
pub const DEFAULT_MAX_CONCURRENT_RUNS: u32 = 3;

/// The file the limits live in, within the [sessions module](crate::Agents::limits).
const SETTINGS_FILE: &str = "settings.toml";

/// The `sessions/settings.toml` shape: how many runs may be live at once, overall and per project.
/// Unknown fields are ignored, so an older store keeps loading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunLimits {
    /// The most runs that may be live at once, across every agent and backend. `0` lifts the cap
    /// entirely — for someone who has decided their machine is the only limit that matters.
    pub max_concurrent_runs: u32,
    /// Per-project caps, by project id: the most of [`max_concurrent_runs`](Self::max_concurrent_runs)
    /// that one project's agents may hold at once. A project with no entry is bounded only by the
    /// global cap; an entry never lifts that cap, it only narrows it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub projects: BTreeMap<String, u32>,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_concurrent_runs: DEFAULT_MAX_CONCURRENT_RUNS,
            projects: BTreeMap::new(),
        }
    }
}

impl RunLimits {
    /// Read the limits, materializing the file from defaults on first use so it is there to edit.
    /// A corrupt or unreadable file falls back to the defaults: a settings file must never be the
    /// reason nothing can run.
    #[must_use]
    pub fn load(module: &Module) -> Self {
        module
            .file::<Self>(SETTINGS_FILE)
            .load_or_create()
            .unwrap_or_default()
    }

    /// Write the limits back.
    ///
    /// # Errors
    /// [`Error::Config`](crate::Error::Config) if the file can't be written.
    pub fn save(&self, module: &Module) -> Result<()> {
        module.file(SETTINGS_FILE).save(self)?;
        Ok(())
    }

    /// Whether `running` live runs already fill the global cap — the question every launch asks.
    #[must_use]
    pub fn is_full(&self, running: usize) -> bool {
        self.max_concurrent_runs > 0 && running >= self.max_concurrent_runs as usize
    }

    /// This project's own cap, or `None` when it has none of its own. A stored `0` reads as no cap,
    /// so clearing one and never setting one are the same state.
    #[must_use]
    pub fn project_limit(&self, project: &str) -> Option<u32> {
        self.projects.get(project).copied().filter(|max| *max > 0)
    }

    /// Whether `running` live runs of `project` already fill *its* cap. False when it has none.
    #[must_use]
    pub fn project_is_full(&self, project: &str, running: usize) -> bool {
        self.project_limit(project)
            .is_some_and(|max| running >= max as usize)
    }

    /// Set (or, with `0`, clear) one project's cap.
    pub fn set_project(&mut self, project: &str, max_concurrent_runs: u32) {
        if max_concurrent_runs == 0 {
            self.projects.remove(project);
        } else {
            self.projects
                .insert(project.to_string(), max_concurrent_runs);
        }
    }
}

/// What is running right now: the total, and how it divides between projects. Taken in one pass so
/// a launch, a poll, or a page render weighs both caps without walking the store twice.
///
/// A run belongs to the project its agent is *currently* filed under, so moving an agent between
/// projects moves the weight of its live runs with it. Attribution is exact — a sub-project's runs
/// count against that sub-project, never against its parent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunLoad {
    total: usize,
    by_project: BTreeMap<String, usize>,
}

impl RunLoad {
    /// Build the snapshot from the live runs of each agent (`by_agent`) and what project each agent
    /// belongs to (`projects`, the agent list the caller already holds).
    pub(crate) fn new(
        by_agent: &BTreeMap<String, usize>,
        projects: impl Iterator<Item = (String, Option<String>)>,
    ) -> Self {
        let total = by_agent.values().sum();
        let mut by_project: BTreeMap<String, usize> = BTreeMap::new();
        for (agent, project) in projects {
            let Some(project) = project else { continue };
            let live = by_agent.get(&agent).copied().unwrap_or(0);
            if live > 0 {
                *by_project.entry(project).or_default() += live;
            }
        }
        Self { total, by_project }
    }

    /// Every live run, across every agent and backend.
    #[must_use]
    pub fn total(&self) -> usize {
        self.total
    }

    /// The live runs of one project.
    #[must_use]
    pub fn in_project(&self, project: &str) -> usize {
        self.by_project.get(project).copied().unwrap_or(0)
    }

    /// The projects with something running, and how much — for a caller reporting the load rather
    /// than asking about one project.
    #[must_use]
    pub fn projects(&self) -> &BTreeMap<String, usize> {
        &self.by_project
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Module {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-limits-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root).module("sessions")
    }

    #[test]
    fn a_fresh_store_reads_the_default_and_writes_it_down() {
        let module = scratch("default");
        let limits = RunLimits::load(&module);
        assert_eq!(limits.max_concurrent_runs, DEFAULT_MAX_CONCURRENT_RUNS);
        assert!(limits.projects.is_empty());
        assert!(
            module.dir().join(SETTINGS_FILE).exists(),
            "the settings file is materialized so it can be edited by hand"
        );
    }

    #[test]
    fn both_limits_round_trip() {
        let module = scratch("round-trip");
        let mut limits = RunLimits {
            max_concurrent_runs: 7,
            ..RunLimits::default()
        };
        limits.set_project("bugbounty", 2);
        limits.save(&module).expect("save");

        let read = RunLimits::load(&module);
        assert_eq!(read.max_concurrent_runs, 7);
        assert_eq!(read.project_limit("bugbounty"), Some(2));
        assert_eq!(read.project_limit("other"), None);
    }

    #[test]
    fn a_corrupt_file_reads_as_the_default() {
        let module = scratch("corrupt");
        module.ensure_dir().expect("mkdir");
        std::fs::write(module.dir().join(SETTINGS_FILE), "not = [toml").expect("write");
        assert_eq!(
            RunLimits::load(&module).max_concurrent_runs,
            DEFAULT_MAX_CONCURRENT_RUNS
        );
    }

    #[test]
    fn the_cap_fills_at_the_limit_and_zero_means_unlimited() {
        let three = RunLimits {
            max_concurrent_runs: 3,
            ..RunLimits::default()
        };
        assert!(!three.is_full(2));
        assert!(three.is_full(3));
        assert!(three.is_full(4));

        let off = RunLimits {
            max_concurrent_runs: 0,
            ..RunLimits::default()
        };
        assert!(!off.is_full(0));
        assert!(!off.is_full(1_000));
    }

    /// A project cap is its own gate, and clearing one (a `0`) drops the entry rather than
    /// recording a cap of nothing.
    #[test]
    fn a_project_cap_gates_only_that_project_and_clears_to_nothing() {
        let mut limits = RunLimits::default();
        limits.set_project("bugbounty", 2);

        assert!(!limits.project_is_full("bugbounty", 1));
        assert!(limits.project_is_full("bugbounty", 2));
        assert!(
            !limits.project_is_full("mono", 99),
            "a project with no cap of its own is bounded only by the global one"
        );

        limits.set_project("bugbounty", 0);
        assert_eq!(limits.project_limit("bugbounty"), None);
        assert!(limits.projects.is_empty());
    }

    /// The load divides by the agent's project, counts nothing for unfiled agents, and totals
    /// everything.
    #[test]
    fn the_load_divides_live_runs_between_projects() {
        let by_agent: BTreeMap<String, usize> = [
            ("solver".to_string(), 2),
            ("triager".to_string(), 1),
            ("loose".to_string(), 1),
        ]
        .into_iter()
        .collect();
        let projects = [
            ("solver".to_string(), Some("bugbounty".to_string())),
            ("triager".to_string(), Some("bugbounty".to_string())),
            ("loose".to_string(), None),
            // An idle agent contributes nothing, and never invents a project row.
            ("idle".to_string(), Some("mono".to_string())),
        ];

        let load = RunLoad::new(&by_agent, projects.into_iter());
        assert_eq!(load.total(), 4);
        assert_eq!(load.in_project("bugbounty"), 3);
        assert_eq!(load.in_project("mono"), 0);
        assert_eq!(load.projects().len(), 1);
    }
}
