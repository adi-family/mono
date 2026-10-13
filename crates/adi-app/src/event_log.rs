//! Keeps the shared event log healthy in the background: relays whatever is still sitting in
//! `tasks.db`'s own outbox table (see `adi_events::outbox`) into the shared log, and reclaims
//! space from events every subscriber has long since passed.
//!
//! Neither half depends on this loop to be *correct* — [`adi_tasks::Tasks::with_doc`] already
//! relays right after every commit, and [`adi_events::Events::retain`] only ever drops what every
//! registered cursor has passed — but a store nothing writes to for a while (the app was down and
//! nothing else touched `tasks.db` either) would otherwise leave its outbox backlog unrelayed
//! until the next write happens to come along. This is that next write, on a schedule, with
//! nobody waiting on it.

use std::time::Duration;

use adi_events::Events;
use adi_tasks::Tasks;
use tracing::{info, warn};

/// How often the sweep runs. Relaying and retention are both cheap, indexed operations, and
/// neither is latency-sensitive the way event *delivery* is (see `adi-triggers`' one-second
/// dispatch tick) — this exists only to cover the gap between "nothing happened to trigger a
/// relay" and "something eventually will".
const SWEEP: Duration = Duration::from_secs(60);

/// How long an event log entry survives once every registered subscriber has passed it. Chosen
/// generously: nothing here is high-volume enough that 30 days of history is a storage concern,
/// and a 30-day window comfortably outlives any plausible "the app was down" gap a subscriber's
/// cursor would otherwise need to survive. [`Events::retain`] never drops anything a cursor
/// hasn't passed regardless of age, so this is a ceiling, not a guess that could lose a reader
/// its backlog.
const RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Start a detached worker that runs until the process exits.
pub fn start(tasks: Tasks, events: Events) {
    std::thread::spawn(move || run(&tasks, &events));
}

fn run(tasks: &Tasks, events: &Events) {
    loop {
        std::thread::sleep(SWEEP);
        match tasks.relay_pending_events() {
            Ok(0) => {}
            Ok(relayed) => info!(relayed, "relayed a backlog of task events into the shared log"),
            Err(error) => warn!(%error, "couldn't relay pending task events"),
        }
        match events.retain(RETENTION) {
            Ok(0) => {}
            Ok(dropped) => info!(dropped, "retired events every subscriber had passed"),
            Err(error) => warn!(%error, "couldn't apply event log retention"),
        }
    }
}
