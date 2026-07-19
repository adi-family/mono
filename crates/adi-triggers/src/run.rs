//! The run state of a supervised background trigger, published as a file so it can be read
//! from *another process*.
//!
//! Supervision happens inside the app ([`crate::Supervisor`]), but "is my Telegram bot up?" is
//! asked by the API handler and by `adi triggers list` in a separate process. So the supervisor
//! publishes each live trigger's state to `triggers/run/<name>.toml` and refreshes its
//! heartbeat every tick; readers treat a state whose heartbeat has gone quiet as dead. That
//! keeps status honest even when the app is `kill -9`ed and never gets to clean up — no
//! signalling, no pid table, no shared memory.

use serde::{Deserialize, Serialize};

use adi_config::now_unix;

/// Where run states live under the module dir.
pub(crate) const RUN_DIR: &str = "run";

/// How long a heartbeat stays trustworthy. Comfortably above the supervisor's tick so a busy
/// machine never blinks a healthy trigger to "stopped", and short enough that a hard-killed app
/// stops claiming its triggers are up within a few seconds.
pub(crate) const STALE_AFTER: u64 = 15;

/// A background trigger's published run state. Written on start, refreshed each supervisor tick,
/// and removed on a clean stop.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RunState {
    /// The supervised process's pid.
    #[serde(default)]
    pub pid: u32,
    /// When the current process started, as Unix epoch seconds.
    #[serde(default)]
    pub started_at: u64,
    /// How many times the supervisor has relaunched this trigger after an exit. Non-zero is the
    /// signal worth surfacing: the code block keeps dying.
    #[serde(default)]
    pub restarts: u32,
    /// When the supervisor last confirmed this process alive. A state whose heartbeat is older
    /// than [`STALE_AFTER`] describes a supervisor that died without cleaning up.
    #[serde(default)]
    pub heartbeat_at: u64,
    /// The command line that was launched. Recorded so an orphan can be identified before it is
    /// signalled: pids get recycled, and a stale state file must never be licence to kill
    /// whatever now holds that number.
    #[serde(default)]
    pub command: String,
}

impl RunState {
    /// Whether this published state still describes a live process — i.e. whether the
    /// supervisor has confirmed it recently enough to be believed.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.pid > 0 && now_unix().saturating_sub(self.heartbeat_at) <= STALE_AFTER
    }

    /// How long the process has been up, in seconds, or `None` once the state has gone stale.
    #[must_use]
    pub fn uptime_secs(&self) -> Option<u64> {
        self.is_live()
            .then(|| now_unix().saturating_sub(self.started_at))
    }
}

/// A trigger's status as the outside world sees it: its published state when a supervisor is
/// keeping it alive, `None` when nothing is running it.
pub type Status = Option<RunState>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_heartbeat_reads_as_live() {
        let now = now_unix();
        let state = RunState {
            pid: 42,
            started_at: now - 10,
            restarts: 0,
            heartbeat_at: now,
            command: "sh -c true".into(),
        };
        assert!(state.is_live());
        assert_eq!(state.uptime_secs(), Some(10));
    }

    /// The crash-safety property: a supervisor that died without cleaning up leaves a state
    /// file behind, and it must not keep claiming the trigger is up.
    #[test]
    fn a_quiet_heartbeat_reads_as_dead() {
        let state = RunState {
            pid: 42,
            started_at: now_unix() - 600,
            restarts: 0,
            heartbeat_at: now_unix() - (STALE_AFTER + 5),
            command: "sh -c true".into(),
        };
        assert!(!state.is_live());
        assert_eq!(state.uptime_secs(), None);
    }

    #[test]
    fn an_empty_state_is_never_live() {
        assert!(!RunState::default().is_live());
    }
}
