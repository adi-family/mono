//! Delivers platform events to the [event triggers](crate::KIND_EVENT) subscribed to them.
//!
//! Publishers ([`adi_events`]) append event records to a shared, durable log; this dispatcher is
//! one [`SUBSCRIBER`] of that log, catching up from its own cursor every tick and, for each event,
//! firing every enabled `event` trigger whose patterns match — handing the event's payload over as
//! `ADI_PAYLOAD` and its name as `ADI_EVENT`. It is the event-side twin of
//! [`Supervisor`](crate::Supervisor): the supervisor keeps *background* triggers alive, the
//! dispatcher launches *event* triggers on demand. Both run inside the app.
//!
//! Delivery is at-least-once, including across a restart: this dispatcher's cursor is itself a row
//! in the log's own database, so an app that was down for an hour catches up from exactly where it
//! left off rather than from "now". An event with no subscriber is still acked (nothing to retry
//! it for), a fire that fails to spawn is logged and the event is still acked (a permanently broken
//! code block must not wedge the whole log), and a burst is spread across ticks so a backlog can't
//! fork-bomb the machine.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};

use adi_events::{EventRecord, Events};

use crate::{SUBSCRIBER, Triggers};

/// A callback handed every event this dispatcher catches up on, beside the triggers it fires.
///
/// It runs on the dispatch tick, so an observer must return promptly: hand the event to a worker,
/// don't do the work here.
pub type EventObserver = Arc<dyn Fn(&EventRecord) + Send + Sync>;

/// How often the log is polled. Short, because event latency is user-visible ("I created a task,
/// did my hook run?"), and polling with nothing new is a single cheap, indexed query.
const TICK: Duration = Duration::from_secs(1);

/// The most events one tick will catch up on. A log that grew while the app was down is caught up
/// a slice at a time rather than spawning thousands of processes at once; the remainder waits for
/// the next tick, exactly as it would if it had just now been appended.
const MAX_PER_TICK: usize = 200;

/// Catches up on the event log and fires matching event triggers. Cheap to clone behind an `Arc`;
/// hold one for as long as events should be delivered, and [`stop`](Self::stop) it on the way out.
pub struct EventDispatcher {
    triggers: Triggers,
    events: Events,
    /// Told about every event this dispatcher catches up on, so something other than a trigger
    /// can react to one without reading the log itself. `None` for a dispatcher nobody is
    /// listening in on.
    observer: Option<EventObserver>,
    /// Stops the dispatch loop.
    shutdown: watch::Sender<bool>,
    /// Flipped once the loop has exited, so [`stop`](Self::stop) can await a clean end.
    done: watch::Sender<bool>,
}

/// Hand-written because an [`EventObserver`] is a closure, which has nothing to print.
impl std::fmt::Debug for EventDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventDispatcher")
            .field("triggers", &self.triggers)
            .field("events", &self.events)
            .field("observed", &self.observer.is_some())
            .finish_non_exhaustive()
    }
}

impl EventDispatcher {
    /// Start delivering events to `triggers`' event subscribers: spawns the dispatch loop and
    /// returns immediately.
    ///
    /// # Panics
    /// If called outside a tokio runtime.
    #[must_use]
    pub fn start(triggers: Triggers) -> Arc<Self> {
        let (dispatcher, rx) = Self::new(triggers, None);
        tokio::spawn(Arc::clone(&dispatcher).run_loop(rx));
        dispatcher
    }

    /// Start the dispatcher with someone listening in: `observer` is handed every drained event
    /// alongside the triggers it fires. This is how the app wakes [awaiting
    /// runs](adi_events) on an event without a second drainer racing this one for records.
    ///
    /// # Panics
    /// If called outside a tokio runtime.
    #[must_use]
    pub fn start_watched(triggers: Triggers, observer: EventObserver) -> Arc<Self> {
        let (dispatcher, rx) = Self::new(triggers, Some(observer));
        tokio::spawn(Arc::clone(&dispatcher).run_loop(rx));
        dispatcher
    }

    /// A dispatcher that delivers nothing: for hosts that only need to satisfy the interface
    /// (tests, tools that mutate trigger definitions without owning event delivery).
    #[must_use]
    pub fn inert(triggers: Triggers) -> Arc<Self> {
        Self::new(triggers, None).0
    }

    /// The shared state plus the shutdown receiver the loop watches. Touches no runtime.
    fn new(
        triggers: Triggers,
        observer: Option<EventObserver>,
    ) -> (Arc<Self>, watch::Receiver<bool>) {
        let events = Events::with_config(triggers.config().clone());
        let (shutdown, rx) = watch::channel(false);
        let (done, _) = watch::channel(false);
        (
            Arc::new(Self {
                triggers,
                events,
                observer,
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

    /// The dispatch loop: catch up and deliver, then wait for the next tick or shutdown.
    async fn run_loop(self: Arc<Self>, mut rx: watch::Receiver<bool>) {
        loop {
            // Catching up on the log, reading the triggers and firing what matches is all disk
            // work (and, for a match, a spawned process). At one tick a second it would otherwise
            // hold an async worker every second, for as long as the app is up.
            let dispatcher = Arc::clone(&self);
            if let Err(e) = tokio::task::spawn_blocking(move || dispatcher.dispatch_pending()).await
            {
                warn!(error = %e, "dispatching caught-up events failed");
            }
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

    /// Catch up on up to [`MAX_PER_TICK`] events past this dispatcher's own cursor and fire the
    /// event triggers each one matches. Every caught-up event is acked whether or not it had a
    /// subscriber (an unsubscribed event is simply dropped) and whether or not a fire succeeded (a
    /// failed spawn is logged, never retried) — acked one at a time, right after it is handled, so
    /// a crash partway through a batch replays only what that crash actually missed.
    fn dispatch_pending(&self) {
        let caught_up = match self.events.catch_up(SUBSCRIBER, MAX_PER_TICK) {
            Ok(caught_up) => caught_up,
            Err(e) => {
                warn!(error = %e, "couldn't catch up on the event log");
                return;
            }
        };
        if caught_up.is_empty() {
            return;
        }

        // The enabled event triggers, loaded once for this batch. A read failure leaves the
        // cursor where it was so these events are retried next tick rather than silently skipped.
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
                warn!(error = %e, "couldn't read triggers; leaving the cursor where it was");
                return;
            }
        };

        for ev in caught_up {
            // Whoever is listening in hears about the event whether or not a trigger wanted it: an
            // awaiting run is subscribed to the *bus*, not to anyone's trigger.
            if let Some(observer) = self.observer.as_ref() {
                observer(&ev.record);
            }
            // The project this event names, read once from its payload — a subscriber restricted by
            // `trigger_on` fires only for a project in its allowlist (an empty allowlist, the
            // default, fires for every project).
            let project = crate::payload_project(ev.record.payload.as_bytes());
            for t in &subscribers {
                let matches_name = t
                    .manifest
                    .events
                    .iter()
                    .any(|pattern| adi_events::matches(pattern, &ev.record.name));
                if matches_name && t.manifest.allows_project(project.as_deref()) {
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
            if let Err(e) = self.events.ack(SUBSCRIBER, ev.seq) {
                warn!(error = %e, "couldn't advance the event dispatcher's cursor");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_events::Event;
    use serde::Serialize;

    use crate::TriggerManifest;
    use crate::trigger::{KIND_EVENT, RUNTIME_SH};

    #[derive(Serialize)]
    struct ItemCreated {
        id: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        project: Option<&'static str>,
    }

    impl Event for ItemCreated {
        const NAME: &'static str = "test.items.created";
    }

    #[derive(Serialize)]
    struct OtherEvent {}

    impl Event for OtherEvent {
        const NAME: &'static str = "test.other.created";
    }

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
            "on-item",
            "printf '%s|%s' \"$ADI_EVENT\" \"$ADI_PAYLOAD\"",
            &["test.items.*"],
        );
        let bus = Events::with_config(store.config().clone());

        let dispatcher = EventDispatcher::start(store.clone());
        let event = ItemCreated {
            id: "t1",
            project: None,
        };
        let record = event.to_record().expect("item-created record");
        let expected = format!("{}|{}", record.name, record.payload);
        bus.emit_event(&event).expect("emit");

        assert!(
            wait_until(|| store.read_log("on-item").as_deref() == Some(expected.as_str())).await,
            "the subscriber should have fired with event name and payload"
        );
        // The event was acked — this dispatcher's cursor caught up with it.
        assert!(bus.catch_up(SUBSCRIBER, 10).expect("catch up").is_empty());

        dispatcher.stop(Duration::from_secs(2)).await;
    }

    /// An event nobody subscribes to is caught up on and acked, not left to pile up.
    #[tokio::test]
    async fn an_unsubscribed_event_is_dropped() {
        let store = scratch("nosub");
        save_event_trigger(&store, "on-item", "true", &["test.items.*"]);
        let bus = Events::with_config(store.config().clone());

        let dispatcher = EventDispatcher::start(store.clone());
        bus.emit_event(&OtherEvent {}).expect("emit");

        assert!(
            wait_until(|| bus.catch_up(SUBSCRIBER, 10).expect("catch up").is_empty()).await,
            "the unmatched event should be caught up on and dropped"
        );
        assert!(
            store.read_log("on-item").is_none(),
            "the non-matching subscriber must not have fired"
        );

        dispatcher.stop(Duration::from_secs(2)).await;
    }

    /// A `trigger_on` allowlist gates delivery by the event payload's `project`: the trigger fires
    /// for an in-list project, stays silent for one outside the list, and stays silent for an event
    /// that names no project at all.
    #[tokio::test]
    async fn trigger_on_gates_delivery_by_payload_project() {
        let store = scratch("triggeron");
        store
            .save(
                "on-alpha",
                TriggerManifest {
                    kind: KIND_EVENT.into(),
                    runtime: RUNTIME_SH.into(),
                    code: "printf '%s' \"$ADI_PAYLOAD\"".into(),
                    events: vec!["test.items.*".into()],
                    trigger_on: vec!["alpha".into()],
                    ..TriggerManifest::default()
                },
            )
            .expect("save");
        let bus = Events::with_config(store.config().clone());
        let dispatcher = EventDispatcher::start(store.clone());

        // An item in a different project must not fire it, and neither must one naming no project.
        bus.emit_event(&ItemCreated {
            id: "t1",
            project: Some("beta"),
        })
        .expect("emit beta");
        bus.emit_event(&ItemCreated {
            id: "t2",
            project: None,
        })
        .expect("emit none");
        assert!(
            wait_until(|| bus.catch_up(SUBSCRIBER, 10).expect("catch up").is_empty()).await,
            "both non-matching events should be caught up on"
        );
        assert!(
            store.read_log("on-alpha").is_none(),
            "a project-restricted trigger must not fire for another project or an unattributed event"
        );

        // An item in the allowed project fires it, with its payload intact.
        let event = ItemCreated {
            id: "t3",
            project: Some("alpha"),
        };
        let record = event.to_record().expect("item-created record");
        bus.emit_event(&event).expect("emit alpha");
        assert!(
            wait_until(|| store.read_log("on-alpha").as_deref() == Some(record.payload.as_str()))
                .await,
            "the trigger should fire for an item in its allowed project"
        );

        dispatcher.stop(Duration::from_secs(2)).await;
    }

    /// A pattern that doesn't match is skipped even when other subscribers do fire.
    #[tokio::test]
    async fn only_matching_patterns_fire() {
        let store = scratch("selective");
        save_event_trigger(&store, "items-only", "printf hit", &["test.items.*"]);
        save_event_trigger(&store, "other-only", "printf hit", &["test.other.**"]);
        let bus = Events::with_config(store.config().clone());

        let dispatcher = EventDispatcher::start(store.clone());
        bus.emit_event(&ItemCreated {
            id: "t1",
            project: None,
        })
        .expect("emit");

        assert!(
            wait_until(|| store.read_log("items-only").as_deref() == Some("hit")).await,
            "the items subscriber should fire"
        );
        // Give the other subscriber every chance to (wrongly) fire before asserting it didn't.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            store.read_log("other-only").is_none(),
            "a non-matching pattern must not fire"
        );

        dispatcher.stop(Duration::from_secs(2)).await;
    }

    /// "App-down replay": events appended while no dispatcher is running at all (not merely one
    /// that hasn't gotten to them yet) are still fired once a dispatcher starts — and a dispatcher
    /// that starts *again* later (a fresh [`EventDispatcher`] instance, exactly what a restarted
    /// app constructs) resumes from the cursor the log kept, not from whatever is newest.
    #[tokio::test]
    async fn events_appended_with_no_dispatcher_running_are_caught_up_on_a_later_start() {
        let store = scratch("app-down");
        save_event_trigger(
            &store,
            "on-item",
            "printf '%s' \"$ADI_PAYLOAD\"",
            &["test.items.*"],
        );
        let bus = Events::with_config(store.config().clone());

        // The "app" is down: nothing is draining the log while this is appended.
        let first_event = ItemCreated {
            id: "t1",
            project: None,
        };
        let first_payload = first_event.to_record().expect("record").payload;
        bus.emit_event(&first_event).expect("emit while nothing was running");
        assert!(store.read_log("on-item").is_none(), "nothing could have fired yet");

        // The app "starts" — a dispatcher that has never run before on this store.
        let first = EventDispatcher::start(store.clone());
        assert!(
            wait_until(|| store.read_log("on-item").as_deref() == Some(first_payload.as_str()))
                .await,
            "the backlog from before the dispatcher existed should still fire"
        );
        first.stop(Duration::from_secs(2)).await;

        // The app "restarts": a second, independent `EventDispatcher` for the same store. A fresh
        // event appended now must fire exactly once, through the cursor the log — not this
        // process — remembers.
        let second_event = ItemCreated {
            id: "t2",
            project: None,
        };
        let second_payload = second_event.to_record().expect("record").payload;
        bus.emit_event(&second_event).expect("emit after the restart");
        let second = EventDispatcher::start(store.clone());
        assert!(
            wait_until(|| store.read_log("on-item").as_deref() == Some(second_payload.as_str()))
                .await,
            "a fresh dispatcher instance must pick up where the log's cursor left off, not replay \
             the first event again"
        );
        second.stop(Duration::from_secs(2)).await;

        // And the original event from before either dispatcher existed was not re-delivered —
        // acking is durable across the restart too, not just catching up.
        assert_eq!(
            bus.cursor(SUBSCRIBER).expect("cursor"),
            2,
            "both events were acked exactly once each"
        );
    }
}
