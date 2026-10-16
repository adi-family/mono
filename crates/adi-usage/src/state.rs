//! Everything this crate remembers about one install: `~/.adi/mono/analytics/state.toml`.
//!
//! Separate from [`crate::settings`] on purpose — turning usage events off must never touch this
//! file (the plan is explicit: an existing `install_id` survives being switched off, and is
//! reused rather than re-minted if it is switched back on). Only [`forget`] clears it.

use adi_config::Module;
use serde::{Deserialize, Serialize};

const STATE_FILE: &str = "state.toml";

/// `analytics/state.toml`'s whole shape.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// A random UUID, minted once, the first time usage events run while on. `None` before that
    /// — including for the whole life of an install that keeps usage events off.
    pub install_id: Option<String>,
    /// Unix seconds [`install_id`](Self::install_id) was minted — the start of the first-7-days
    /// window `agent_run_started`/`agent_run_finished` are gated on.
    #[serde(default)]
    pub first_launch_at: u64,
    /// Whether `onboarding_completed` has ever gone out. The root agent's definition may be saved
    /// many times over an install's life (every "Reconfigure"); this event is the first one,
    /// once, ever — so unlike the 7-day window above, this is a flag rather than a timestamp.
    #[serde(default)]
    pub onboarding_sent: bool,
    /// The local calendar day (`YYYY-MM-DD`) [`runs_human`](Self::runs_human) and its two
    /// counterparts are counting. Empty before the first run of any kind. A day that does not
    /// match "today" is what [`crate::dispatch`] reads as "flush `app_daily_active` for the day
    /// that just ended, then start counting a new one".
    #[serde(default)]
    pub day: String,
    #[serde(default)]
    pub runs_human: u64,
    #[serde(default)]
    pub runs_agent: u64,
    #[serde(default)]
    pub runs_automation: u64,
}

impl State {
    /// Load from disk, or [`State::default`] if it has never been written — never materializes
    /// the file, unlike [`crate::settings::Settings::load`], since the overwhelming majority of
    /// reads (every tick, while usage events are off) have nothing to write down.
    #[must_use]
    pub fn load(module: &Module) -> Self {
        module.file(STATE_FILE).load_or_default().unwrap_or_default()
    }

    /// # Errors
    /// [`adi_config::Error::Io`] on a write failure.
    pub fn save(&self, module: &Module) -> adi_config::Result<()> {
        module.file(STATE_FILE).save(self)
    }
}

/// `adi-mono analytics forget` / the panel's "Forget install ID" button: clear everything this
/// install has accumulated, `install_id` included. A fresh one is minted only the next time usage
/// events run while on — never here, so turning the switch back on right after does not silently
/// hand back the id this call meant to retire.
///
/// # Errors
/// [`adi_config::Error::Io`] on a write failure.
pub fn forget(module: &Module) -> adi_config::Result<()> {
    State::default().save(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Module {
        let root = std::env::temp_dir().join(format!(
            "adi-usage-state-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root).module("analytics")
    }

    #[test]
    fn a_fresh_store_has_no_install_id_and_writes_nothing() {
        let module = scratch("fresh");
        let state = State::load(&module);
        assert_eq!(state, State::default());
        assert!(
            !module.dir().join(STATE_FILE).exists(),
            "reading must not materialize the file"
        );
    }

    #[test]
    fn save_then_load_round_trips() {
        let module = scratch("roundtrip");
        let state = State {
            install_id: Some("1234abcd-0000-0000-0000-000000000000".to_string()),
            first_launch_at: 1_700_000_000,
            onboarding_sent: true,
            day: "2026-10-16".to_string(),
            runs_human: 3,
            runs_agent: 1,
            runs_automation: 0,
        };
        state.save(&module).expect("save");
        assert_eq!(State::load(&module), state);
    }

    #[test]
    fn forget_clears_everything_including_the_install_id() {
        let module = scratch("forget");
        State {
            install_id: Some("1234abcd-0000-0000-0000-000000000000".to_string()),
            first_launch_at: 1_700_000_000,
            onboarding_sent: true,
            day: "2026-10-16".to_string(),
            runs_human: 3,
            runs_agent: 0,
            runs_automation: 0,
        }
        .save(&module)
        .expect("save");

        forget(&module).expect("forget");
        assert_eq!(State::load(&module), State::default());
    }
}
