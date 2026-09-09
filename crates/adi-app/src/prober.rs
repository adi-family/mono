//! The LLM prober worker — the app's clock for backends that ran out.
//!
//! A [hold](adi_agents::llm::Holds) says "this credential is spent until 14:00". With nothing on a
//! timer, the first chat to start a turn after 14:00 is what finds out whether that was true — and
//! that is exactly what the design forbids: *a chat turn must never be what discovers a backend is
//! back*. So a sweep runs here instead, asks each due backend one throwaway question, and writes the
//! answer into the shared hold store before any conversation needs it.
//!
//! The judgement lives in [`adi_agents::llm::Prober`]; what lives here is the only thing that crate
//! cannot have — a clock that runs for as long as the panel does.
//!
//! # Why here and not in the hive
//!
//! This is a supervised loop with no socket, and the hive supervises **by the port**: a service that
//! declares none is started once and then never watched again, so a prober that died at 03:00 would
//! stay dead and nothing would say so. A background trigger has the mirror-image problem — disabling
//! one does not stop the loop already running. The app already owns three workers of exactly this
//! shape (the trigger supervisor, the event dispatcher, the await worker); this is the fourth, and
//! it starts and stops with the panel like the rest of them.

use std::time::Duration;

use adi_agents::Agents;
use adi_agents::llm::holds::clock;
use adi_agents::llm::{Checked, Prober, Verdict};
use tracing::{debug, info, warn};

/// How often the worker wakes to ask whether a sweep is due.
///
/// Not the sweep interval — that is `probe_every` in `llm/settings.toml`, and it is minutes. This is
/// the granularity at which a *change* to that setting is noticed, which is why it is short: an
/// operator who turns probing off, or down to a minute, should not wait out the old interval first.
/// A wakeup that finds nothing due reads one small TOML file and goes back to sleep.
const POLL: Duration = Duration::from_secs(15);

/// Start the prober over `agents`' store root.
///
/// The thread is detached and runs until the process ends, which is the honest lifetime: there is
/// nothing to shut down cleanly — a sweep either finished writing its verdict or never started one.
pub fn start(agents: Agents) {
    std::thread::spawn(move || run(&agents));
}

/// Sweep whenever the configured interval has elapsed, re-reading the interval every wakeup.
///
/// A hold that is already due when the app starts waits out one full interval rather than being
/// probed the instant the panel comes up: a probe is a real billed request, and an app being
/// restarted in a loop must not turn into one request per restart against somebody's subscription.
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
            // A sweep that could not read the store is a bad minute, not a reason to stop having a
            // prober: the next one re-opens everything from scratch.
            Err(error) => warn!(%error, "the llm prober could not sweep its holds"),
        }
    }
}

/// One wakeup's arithmetic: whether this is a sweep, and what the clock reads afterwards.
///
/// Separated from the loop so the rule can be read and tested without waiting on a real clock — and
/// there is a rule worth testing here, because `every` may change between any two wakeups.
fn tick(waited: Duration, every: Option<Duration>) -> (bool, Duration) {
    // Probing is switched off. Hold the clock at zero, so switching it back on waits a full interval
    // instead of firing the moment the setting is saved.
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

/// Say what the sweep found, at the volume each answer deserves.
///
/// A backend coming back is the line somebody goes looking for, so it is `info`. Still-limited and
/// unprobeable are the ordinary case and repeat every interval for as long as the hold stands — at
/// `info` they would bury the one line that matters. A probe that failed for a reason that is *not*
/// a limit is a warning: it means a backend is being held out of every agent's chain by something
/// nobody has classified.
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

    /// A short interval sweeps on the first wakeup that reaches it, and the clock starts again from
    /// zero — so a five-minute setting costs one sweep every five minutes, not one per poll after
    /// the first.
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

    /// An interval shorter than the poll cannot ask for more than one sweep per wakeup — the loop's
    /// floor, and the reason a `probe_every` of one second is harmless rather than a request storm.
    #[test]
    fn an_interval_below_the_poll_still_sweeps_only_once_per_wakeup() {
        assert_eq!(tick(Duration::ZERO, Some(Duration::from_secs(1))), (
            true,
            Duration::ZERO
        ));
    }

    /// Switched off means no sweep at all, and the clock does not quietly run up in the meantime:
    /// turning probing back on waits out a full interval rather than firing the moment it is saved.
    #[test]
    fn probing_switched_off_neither_sweeps_nor_banks_the_wait() {
        let (sweep, waited) = tick(POLL * 4, None);
        assert!(!sweep);
        assert_eq!(waited, Duration::ZERO);
    }
}
