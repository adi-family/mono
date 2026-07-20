//! Delivers platform events to the [event triggers](crate::KIND_EVENT) subscribed to them.
//!
//! Publishers ([`adi_events`]) drop event records into a shared spool; this dispatcher polls that
//! spool and, for each event, fires every enabled `event` trigger whose patterns match — handing
//! the event's payload over as `ADI_PAYLOAD` and its name as `ADI_EVENT`. It is the event-side
//! twin of [`Supervisor`](crate::Supervisor): the supervisor keeps *background* triggers alive,
//! the dispatcher launches *event* triggers on demand. Both run inside the app.
//!
//! Delivery is at-least-once while the app is up and best-effort otherwise: an event with no
//! subscriber is dropped, a fire that fails to spawn is logged and the event is still consumed
//! (a permanently broken code block must not wedge the whole spool), and a burst is spread across
//! ticks so a backlog can't fork-bomb the machine.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};

use adi_events::Events;

use crate::Triggers;

/// How often the spool is drained. Short, because event latency is user-visible ("I created a
/// task, did my hook run?"), and draining an empty spool is a single cheap directory read.
const TICK: Duration = Duration::from_secs(1);

/// The most events one tick will fire. A spool that filled while the app was down (up to
/// `adi_events`' own cap) is drained a slice at a time rather than spawning thousands of
/// processes at once; the remainder waits on disk for the next tick.
const MAX_PER_TICK: usize = 200;

/// Drains the event spool and fires matching event triggers. Cheap to clone behind an `Arc`; hold
/// one for as long as events should be delivered, and [`stop`](Self::stop) it on the way out.
#[derive(Debug)]
pub struct EventDispatcher {
    triggers: Triggers,
    events: Events,
    /// Stops the dispatch loop.
    shutdown: watch::Sender<bool>,
    /// Flipped once the loop has exited, so [`stop`](Self::stop) can await a clean end.
    done: watch::Sender<bool>,
}

impl EventDispatcher {
    /// Start delivering events to `triggers`' event subscribers: spawns the dispatch loop and
    /// returns immediately.
    ///
    /// # Panics
    /// If called outside a tokio runtime.
    #[must_use]
    pub fn start(triggers: Triggers) -> Arc<Self> {
        let (dispatcher, rx) = Self::new(triggers);
        tokio::spawn(Arc::clone(&dispatcher).run_loop(rx));
        dispatcher
    }

    /// A dispatcher that delivers nothing: for hosts that only need to satisfy the interface
    /// (tests, tools that mutate trigger definitions without owning event delivery).
    #[must_use]
    pub fn inert(triggers: Triggers) -> Arc<Self> {
        Self::new(triggers).0
    }

    /// The shared state plus the shutdown receiver the loop watches. Touches no runtime.
    fn new(triggers: Triggers) -> (Arc<Self>, watch::Receiver<bool>) {
        let events = Events::with_config(triggers.config().clone());
        let (shutdown, rx) = watch::channel(false);
        let (done, _) = watch::channel(false);
        (
            Arc::new(Self {
                triggers,
                events,
                shutdown,
                done,
            }),
            rx,
        )
    }

    /// Signal the loop to stop, without waiting.
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    /// Stop the dispatch loop and wait for it to actually exit, up to `grace`. Unlike the
    /// supervisor there are no child processes to reap — fired event triggers are detached, own
    /// their lifetime, and are the same one-off launches a manual ▶ Fire produces.
    pub async fn stop(&self, grace: Duration) {
        let mut done = self.done.subscribe();
        if self.shutdown.send(true).is_err() {
            return;
        }
        let _ = tokio::time::timeout(grace, done.changed()).await;
    }

    /// The dispatch loop: drain and deliver, then wait for the next tick or shutdown.
    async fn run_loop(self: Arc<Self>, mut rx: watch::Receiver<bool>) {
        loop {
            self.dispatch_pending();
            tokio::select! {
                () = tokio::time::sleep(TICK) => {}
                _ = rx.changed() => break,
            }
            if *rx.borrow_and_update() {
                break;
            }
        }
        let _ = self.done.send(true);
    }

    /// Drain up to [`MAX_PER_TICK`] spooled events and fire the event triggers each one matches.
    /// Every drained event is removed from the spool whether or not it had a subscriber (an
    /// unsubscribed event is simply dropped) and whether or not a fire succeeded (a failed spawn
    /// is logged, never retried).
    fn dispatch_pending(&self) {
        let spooled = match self.events.drain() {
            Ok(spooled) => spooled,
            Err(e) => {
                warn!(error = %e, "couldn't drain the event spool");
                return;
            }
        };
        if spooled.is_empty() {
            return;
        }

        // The enabled event triggers, loaded once for this batch. A read failure leaves the spool
        // untouched so the events are retried next tick rather than silently dropped.
        let subscribers: Vec<crate::Trigger> = match self.triggers.list() {
            Ok(list) => list
                .into_iter()
                .filter(|t| {
                    t.manifest.enabled
                        && t.manifest.is_event()
                        && !t.manifest.code.trim().is_empty()
                        && !t.manifest.events.is_empty()
                })
                .collect(),
            Err(e) => {
                warn!(error = %e, "couldn't read triggers; leaving events spooled");
                return;
            }
        };

        for ev in spooled.into_iter().take(MAX_PER_TICK) {
            for t in &subscribers {
                if t.manifest
                    .events
                    .iter()
                    .any(|pattern| adi_events::matches(pattern, &ev.record.name))
                {
                    match self.triggers.fire_event(
                        &t.name,
                        &ev.record.name,
                        Some(ev.record.payload.as_bytes()),
                    ) {
                        Ok(firing) => {
                            info!(trigger = %t.name, event = %ev.record.name, pid = firing.pid, "event trigger fired");
                        }
                        Err(e) => {
                            warn!(trigger = %t.name, event = %ev.record.name, error = %e, "event trigger failed to fire");
                        }
                    }
                }
            }
            if let Err(e) = self.events.remove(&ev.path) {
                warn!(error = %e, "couldn't remove delivered event from the spool");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::TriggerManifest;
    use crate::trigger::{KIND_EVENT, RUNTIME_SH};

    fn scratch(tag: &str) -> Triggers {
        let root = std::env::temp_dir().join(format!(
            "adi-triggers-dispatch-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Triggers::with_config(adi_config::Config::with_root(root))
    }

    fn save_event_trigger(store: &Triggers, name: &str, code: &str, patterns: &[&str]) {
        store
            .save(
                name,
                TriggerManifest {
                    kind: KIND_EVENT.into(),
                    runtime: RUNTIME_SH.into(),
                    code: code.into(),
                    events: patterns.iter().map(|s| (*s).to_string()).collect(),
                    ..TriggerManifest::default()
                },
            )
            .expect("save");
    }

    async fn wait_until(mut pred: impl FnMut() -> bool) -> bool {
        for _ in 0..300 {
            if pred() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// The core contract: an emitted event whose name matches a trigger's pattern fires that
    /// trigger with the payload (as `ADI_PAYLOAD`) and the concrete name (as `ADI_EVENT`).
    #[tokio::test]
    async fn a_matching_event_fires_its_subscriber() {
        let store = scratch("match");
        save_event_trigger(
            &store,
            "on-task",
            "printf '%s|%s' \"$ADI_EVENT\" \"$ADI_PAYLOAD\"",
            &["adi.tasks.*"],
        );
        let bus = Events::with_config(store.config().clone());

        let dispatcher = EventDispatcher::start(store.clone());
        bus.emit("adi.tasks.created", r#"{"id":"t1"}"#)
            .expect("emit");

        assert!(
            wait_until(|| store.read_log("on-task").as_deref()
                == Some("adi.tasks.created|{\"id\":\"t1\"}"))
            .await,
            "the subscriber should have fired with event name and payload"
        );
        // The event was consumed from the spool.
        assert!(bus.drain().expect("drain").is_empty());

        dispatcher.stop(Duration::from_secs(2)).await;
    }

    /// An event nobody subscribes to is drained and dropped, not left to pile up.
    #[tokio::test]
    async fn an_unsubscribed_event_is_dropped() {
        let store = scratch("nosub");
        save_event_trigger(&store, "on-task", "true", &["adi.tasks.*"]);
        let bus = Events::with_config(store.config().clone());

        let dispatcher = EventDispatcher::start(store.clone());
        bus.emit("adi.agents.run.started", "{}").expect("emit");

        assert!(
            wait_until(|| bus.drain().expect("drain").is_empty()).await,
            "the unmatched event should be consumed and dropped"
        );
        assert!(
            store.read_log("on-task").is_none(),
            "the non-matching subscriber must not have fired"
        );

        dispatcher.stop(Duration::from_secs(2)).await;
    }

    /// A pattern that doesn't match is skipped even when other subscribers do fire.
    #[tokio::test]
    async fn only_matching_patterns_fire() {
        let store = scratch("selective");
        save_event_trigger(&store, "tasks-only", "printf hit", &["adi.tasks.*"]);
        save_event_trigger(&store, "agents-only", "printf hit", &["adi.agents.**"]);
        let bus = Events::with_config(store.config().clone());

        let dispatcher = EventDispatcher::start(store.clone());
        bus.emit("adi.tasks.created", "{}").expect("emit");

        assert!(
            wait_until(|| store.read_log("tasks-only").as_deref() == Some("hit")).await,
            "the tasks subscriber should fire"
        );
        // Give the agents subscriber every chance to (wrongly) fire before asserting it didn't.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            store.read_log("agents-only").is_none(),
            "a non-matching pattern must not fire"
        );

        dispatcher.stop(Duration::from_secs(2)).await;
    }
}
