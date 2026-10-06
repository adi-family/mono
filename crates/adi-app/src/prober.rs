//! Probe held LLM backends in the background so chat turns never discover recovery.
//!
//! The app owns this loop because the hive supervises services by port and this worker has none.

use std::time::Duration;

use adi_agents::Agents;
use adi_agents::llm::holds::clock;
use adi_agents::llm::{Checked, Prober, Verdict};
use tracing::{debug, info, warn};

/// Poll settings independently of `probe_every` so interval changes take effect promptly.
const POLL: Duration = Duration::from_secs(15);

/// Start a detached worker that runs until the process exits.
pub fn start(agents: Agents) {
    std::thread::spawn(move || run(&agents));
}

/// Wait a full interval after startup to avoid repeated billed probes in a restart loop.
fn run(agents: &Agents) {
    let prober = Prober::with_config(agents.config().clone());
    let mut waited = Duration::ZERO;
    loop {
        std::thread::sleep(POLL);
        let sweep;
        (sweep, waited) = tick(waited, prober.interval());
        if !sweep {
            continue;
        }
        match prober.sweep() {
            Ok(swept) => report(&swept),
            Err(error) => warn!(%error, "the llm prober could not sweep its holds"),
        }
    }
}

fn tick(waited: Duration, every: Option<Duration>) -> (bool, Duration) {
    // Reset while disabled so re-enabling waits a full interval.
    let Some(every) = every else {
        return (false, Duration::ZERO);
    };
    let waited = waited.saturating_add(POLL);
    if waited < every {
        (false, waited)
    } else {
        (true, Duration::ZERO)
    }
}

fn report(swept: &[Checked]) {
    for check in swept {
        let credential = &check.key.credential;
        let model = if check.key.model.is_empty() {
            "*"
        } else {
            &check.key.model
        };
        let backend = check.backend.as_deref().unwrap_or("-");
        match &check.verdict {
            Verdict::Back => info!(
                %credential, %model, %backend,
                "a held llm backend answered again — the hold is released"
            ),
            Verdict::StillOut { until, reason } => debug!(
                %credential, %model, %backend, until = %clock(*until), %reason,
                "still limited; the hold was pushed out"
            ),
            Verdict::Failed { error } => warn!(
                %credential, %model, %backend, %error,
                "a probe failed, and not with a limit — the hold stands"
            ),
            Verdict::Unreachable { reason } => debug!(
                %credential, %model, %backend, %reason,
                "nothing can probe this hold; it expires on its own deadline"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_sweeps_once_an_interval_and_starts_over() {
        let every = Some(POLL * 2);
        let (sweep, waited) = tick(Duration::ZERO, every);
        assert!(!sweep, "one poll into a two-poll interval");
        assert_eq!(waited, POLL);

        let (sweep, waited) = tick(waited, every);
        assert!(sweep, "the interval is up");
        assert_eq!(waited, Duration::ZERO, "and the wait starts again");
    }

    #[test]
    fn an_interval_below_the_poll_still_sweeps_only_once_per_wakeup() {
        assert_eq!(tick(Duration::ZERO, Some(Duration::from_secs(1))), (
            true,
            Duration::ZERO
        ));
    }

    #[test]
    fn probing_switched_off_neither_sweeps_nor_banks_the_wait() {
        let (sweep, waited) = tick(POLL * 4, None);
        assert!(!sweep);
        assert_eq!(waited, Duration::ZERO);
    }
}
