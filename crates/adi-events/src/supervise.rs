//! Loops that outlive their own failures, and say that they failed.
//!
//! A service's background loop — the await worker, the event dispatcher — is a thread or a task
//! nobody is joined on. When one panics it is simply gone: the process stays up, its port still
//! answers, every status reads green, and whatever the loop did never happens again. On
//! 2026-11-24 a full disk did that to four loops of one app in one second, and for eleven hours
//! nothing said so.
//!
//! Two things here are for that:
//!
//! * [`thread`] and [`task`] run a loop and start it again when it panics, after a pause, for as
//!   long as the process lives. Returning normally is the loop's own way to stop.
//! * [`incident`] records that it happened. Recording can fail for the same reason the loop did —
//!   that day neither the log nor the diagnostics store could be written — so an incident that
//!   could not be stored is kept in memory and [`flush`] stores it later. Repeats of one loop's
//!   failure fold into a single record with a count, since a loop restarting once a second into
//!   the same full disk would otherwise write hundreds.

use std::any::Any;
use std::collections::BTreeMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::Duration;

use adi_config::now_unix;
use serde_json::json;

use crate::Events;
use crate::diagnostics::{Diagnostic, Level, complain};

/// How long a loop that panicked waits before it starts again. Long enough that a loop failing on
/// every start does not spin, short enough that one bad second costs one second.
pub const RESTART_PAUSE: Duration = Duration::from_secs(1);

/// This process's name, as diagnostics record it — `adi-app`, `adi-hive`.
fn process_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| {
                exe.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "adi".to_string())
    })
}

/// What a panic carried, as text.
#[must_use]
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panicked with a non-string payload".to_string())
}

/// One loop's failures since they were last stored.
#[derive(Debug, Clone)]
struct Incident {
    message: String,
    first_at: u64,
    last_at: u64,
    count: u64,
}

/// Incidents not yet in the diagnostics store, by loop name.
static UNSTORED: Mutex<BTreeMap<String, Incident>> = Mutex::new(BTreeMap::new());

fn unstored() -> std::sync::MutexGuard<'static, BTreeMap<String, Incident>> {
    UNSTORED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Record that `name` — a loop, or one pass of one — panicked with `message`, and try to store
/// it.
///
/// Never panics and never waits on the outcome: what cannot be stored now is stored by a later
/// [`flush`].
pub fn incident(store: &Events, name: &str, message: &str) {
    let now = now_unix();
    {
        let mut pending = unstored();
        let entry = pending.entry(name.to_string()).or_insert_with(|| Incident {
            message: message.to_string(),
            first_at: now,
            last_at: now,
            count: 0,
        });
        entry.count += 1;
        entry.last_at = now;
    }
    complain(&format!(
        "{}: {name} panicked and is started again: {message}",
        process_name()
    ));
    flush(store);
}

/// Store every incident that has not been stored yet. Cheap when there is none, so a loop may
/// call it on every pass; what still cannot be stored stays for the next call.
pub fn flush(store: &Events) {
    let pending: Vec<(String, Incident)> = {
        let pending = unstored();
        if pending.is_empty() {
            return;
        }
        pending
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };
    for (name, seen) in pending {
        let record = Diagnostic {
            level: Level::Error,
            source: process_name().to_string(),
            area: "supervisor".into(),
            message: if seen.count == 1 {
                format!("{name} panicked and was started again: {}", seen.message)
            } else {
                format!(
                    "{name} panicked {} times and was started again each time: {}",
                    seen.count, seen.message
                )
            },
            context: json!({
                "loop": name,
                "count": seen.count,
                "first_at": seen.first_at,
                "last_at": seen.last_at,
                "pid": std::process::id(),
            }),
            event_key: None,
        };
        let stored = catch_unwind(AssertUnwindSafe(|| store.report(&record).is_ok()));
        if !matches!(stored, Ok(true)) {
            continue;
        }
        // Only what was stored is forgotten: a failure that arrived while this one was being
        // written keeps its own count for the next flush.
        let mut pending = unstored();
        if let Some(current) = pending.get_mut(&name) {
            if current.count <= seen.count {
                pending.remove(&name);
            } else {
                current.count -= seen.count;
                current.first_at = seen.last_at;
            }
        }
    }
}

/// Run `body` on a thread named `name`, starting it again whenever it panics.
///
/// `body` returning is the loop ending on purpose — its channel closed, the process shutting down
/// — and ends the thread. The thread is named so a thread list (`/proc/<pid>/task/*/comm`) shows
/// which loops a live process still has; an unnamed one reads as the process's own name, and its
/// absence proves nothing.
///
/// # Panics
/// If the operating system refuses to start a thread.
pub fn thread(
    store: Events,
    name: &'static str,
    mut body: impl FnMut() + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            loop {
                match catch_unwind(AssertUnwindSafe(&mut body)) {
                    Ok(()) => break,
                    Err(payload) => {
                        incident(&store, name, &panic_message(payload.as_ref()));
                        std::thread::sleep(RESTART_PAUSE);
                    }
                }
            }
        })
        .expect("the operating system refused to start a supervised thread")
}

/// A future that resolves to `Err` with the panic's payload instead of unwinding into whatever
/// polled it.
struct Caught<T>(Pin<Box<dyn Future<Output = T> + Send>>);

impl<T> Future for Caught<T> {
    type Output = Result<T, Box<dyn Any + Send>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match catch_unwind(AssertUnwindSafe(|| self.0.as_mut().poll(cx))) {
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => Poll::Ready(Err(payload)),
        }
    }
}

/// The async twin of [`thread`]: drive the future `start` makes, and make a new one whenever it
/// panics, after `pause` — which the caller supplies because this crate has no runtime, and so no
/// timer, of its own (`|| tokio::time::sleep(supervise::RESTART_PAUSE)`).
///
/// The loop's future resolving is it ending on purpose. Spawn the result; it is one task.
pub async fn task<F, P>(
    store: Events,
    name: &'static str,
    mut start: impl FnMut() -> F + Send,
    mut pause: impl FnMut() -> P + Send,
) where
    F: Future<Output = ()> + Send + 'static,
    P: Future<Output = ()> + Send,
{
    loop {
        match Caught(Box::pin(start())).await {
            Ok(()) => break,
            Err(payload) => {
                incident(&store, name, &panic_message(payload.as_ref()));
                pause().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Query;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch(tag: &str) -> Events {
        let dir =
            std::env::temp_dir().join(format!("adi-events-supervise-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Events::with_config(adi_config::Config::with_root(dir))
    }

    fn recorded(store: &Events, name: &str) -> Vec<String> {
        store
            .diagnostics(&Query::default())
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.diagnostic.context["loop"].as_str() == Some(name))
            .map(|r| r.diagnostic.message)
            .collect()
    }

    /// The whole point: a loop that panics keeps running, and the panic is on record.
    #[test]
    fn a_thread_that_panics_is_started_again_and_the_panic_is_recorded() {
        let store = scratch("thread");
        let runs = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&runs);
        let handle = thread(store.clone(), "flaky-thread", move || {
            assert!(
                seen.fetch_add(1, Ordering::SeqCst) > 0,
                "failed printing to stderr: No space left on device"
            );
        });
        handle.join().expect("the supervisor itself never panics");

        assert_eq!(runs.load(Ordering::SeqCst), 2, "ran, panicked, ran again");
        let messages = recorded(&store, "flaky-thread");
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].contains("No space left on device"),
            "{messages:?}"
        );
    }

    /// 2026-11-24 exactly: the store could not be written while the loop was failing. The record
    /// has to survive that and land once the store works again, as one line with a count.
    #[test]
    fn an_incident_that_cannot_be_stored_is_kept_and_stored_later_with_its_count() {
        let root = std::env::temp_dir().join(format!(
            "adi-events-supervise-unwritable-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&root);
        // A file where the store's directory should be: every write to it fails.
        std::fs::write(&root, b"not a directory").unwrap();
        let broken = Events::with_config(adi_config::Config::with_root(&root));

        for _ in 0..3 {
            incident(&broken, "starved-loop", "disk I/O error");
        }
        assert!(
            unstored().contains_key("starved-loop"),
            "nothing could be stored, so it must still be held"
        );

        std::fs::remove_file(&root).unwrap();
        let working = Events::with_config(adi_config::Config::with_root(&root));
        flush(&working);

        let messages = recorded(&working, "starved-loop");
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains("3 times"), "{messages:?}");
        assert!(
            !unstored().contains_key("starved-loop"),
            "stored, so forgotten"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A minimal executor: these futures never wait on anything, so polling until ready is enough
    /// and keeps this crate free of a runtime even in its tests.
    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let mut future = Box::pin(future);
        let mut cx = Context::from_waker(std::task::Waker::noop());
        loop {
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
        }
    }

    #[test]
    fn a_task_that_panics_is_started_again_and_the_panic_is_recorded() {
        let store = scratch("task");
        let runs = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&runs);
        block_on(task(
            store.clone(),
            "flaky-task",
            move || {
                let seen = Arc::clone(&seen);
                async move {
                    assert!(seen.fetch_add(1, Ordering::SeqCst) > 0, "boom in a task");
                }
            },
            || async {},
        ));

        assert_eq!(runs.load(Ordering::SeqCst), 2, "ran, panicked, ran again");
        let messages = recorded(&store, "flaky-task");
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains("boom in a task"), "{messages:?}");
    }
}
