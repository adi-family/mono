//! `~/.adi/mono/analytics/queue.jsonl`: events queued since the last successful flush.
//!
//! One JSON array, read and rewritten whole rather than appended line by line — simpler to keep
//! correct, and fine given how small this file stays: [`MAX_QUEUED`] caps it outright, and in the
//! steady state the 10-minute flush empties it long before that.

use adi_config::Module;
use serde::{Deserialize, Serialize};

const QUEUE_FILE: &str = "queue.jsonl";

/// One queued item, in the collector's own per-event shape (`collector:src/collect.rs`'s
/// `RECORD_WIRE.itemFields`): a dotted event name, when it happened, a fresh id so a retried POST
/// de-dupes on the collector's side, and this event's own allowed properties — already carrying
/// `version`/`os`/`arch` by the time anything pushes it here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuedEvent {
    pub id: String,
    pub event: String,
    /// Unix milliseconds. Converted to the collector's expected ISO 8601 string only at POST
    /// time ([`crate::client`]), so ageing a queued item out by [`MAX_AGE`] is plain arithmetic.
    pub ts_ms: u64,
    pub properties: serde_json::Value,
}

/// A queued item older than this was never sent in time to be worth sending — the plan's "anything
/// older than 7 days that was never sent is dropped". Applied on read, so a long offline stretch
/// ages items out gradually rather than all at once on the next flush.
pub const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// A safety valve against a machine that stays offline (or opted out and back in) indefinitely:
/// the collector's own cap is 100 events a POST, but nothing stops this on-disk queue from
/// growing while nothing is flushing it. Oldest-first, so a trim always keeps the newest activity.
const MAX_QUEUED: usize = 2_000;

/// Add one item, trimming the oldest entries past [`MAX_QUEUED`] if this push went over.
pub fn enqueue(module: &Module, item: QueuedEvent) {
    let mut items = read(module);
    items.push(item);
    if items.len() > MAX_QUEUED {
        let drop = items.len() - MAX_QUEUED;
        items.drain(0..drop);
    }
    write(module, &items);
}

/// Every queued item, in the order it was pushed. An unreadable or corrupt file reads as empty —
/// a queue is disposable scratch state, never worth failing a flush over.
#[must_use]
pub fn read(module: &Module) -> Vec<QueuedEvent> {
    module
        .read_raw(QUEUE_FILE)
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Drop every item [`MAX_AGE`] older than `now`, and report how many survived the trim — what
/// [`crate::client::flush`] sends is a prefix of this. A write failure (disk full, permissions)
/// is swallowed, same as every other write in this module: a queue is disposable scratch state.
#[must_use]
pub fn drop_stale(module: &Module, now_ms: u64) -> Vec<QueuedEvent> {
    let items = read(module);
    let cutoff = now_ms.saturating_sub(u64::try_from(MAX_AGE.as_millis()).unwrap_or(u64::MAX));
    let (fresh, stale): (Vec<_>, Vec<_>) = items.into_iter().partition(|item| item.ts_ms >= cutoff);
    if !stale.is_empty() {
        write(module, &fresh);
    }
    fresh
}

/// Remove every item whose id is in `sent` — called once a POST of (a prefix of) the queue has
/// actually succeeded. Never clears the whole file: anything queued *after* the batch was built
/// stays, exactly as it would if this flush had simply run a moment earlier.
pub fn remove(module: &Module, sent: &std::collections::HashSet<String>) {
    let items = read(module);
    let remaining: Vec<QueuedEvent> = items.into_iter().filter(|i| !sent.contains(&i.id)).collect();
    write(module, &remaining);
}

fn write(module: &Module, items: &[QueuedEvent]) {
    if let Ok(bytes) = serde_json::to_vec(items) {
        let _ = module.write_raw(QUEUE_FILE, &bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Module {
        let root = std::env::temp_dir().join(format!(
            "adi-usage-queue-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root).module("analytics")
    }

    fn item(id: &str, ts_ms: u64) -> QueuedEvent {
        QueuedEvent {
            id: id.to_string(),
            event: "app_daily_active".to_string(),
            ts_ms,
            properties: serde_json::json!({ "runs_human": 1 }),
        }
    }

    #[test]
    fn enqueue_then_read_round_trips_in_order() {
        let module = scratch("roundtrip");
        enqueue(&module, item("a", 1));
        enqueue(&module, item("b", 2));
        assert_eq!(
            read(&module).into_iter().map(|i| i.id).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn enqueue_trims_the_oldest_past_the_cap() {
        let module = scratch("cap");
        for n in 0..(MAX_QUEUED + 5) {
            enqueue(&module, item(&n.to_string(), n as u64));
        }
        let ids: Vec<String> = read(&module).into_iter().map(|i| i.id).collect();
        assert_eq!(ids.len(), MAX_QUEUED);
        assert_eq!(ids.first().unwrap(), "5", "the oldest five were dropped");
        assert_eq!(ids.last().unwrap(), &(MAX_QUEUED + 4).to_string());
    }

    #[test]
    fn drop_stale_removes_only_what_is_past_max_age() {
        let module = scratch("stale");
        let now_ms = 10 * 24 * 3600 * 1000; // day 10
        enqueue(&module, item("old", 0)); // day 0: 10 days old, past the 7-day window
        enqueue(&module, item("fresh", 4 * 24 * 3600 * 1000)); // day 4: 6 days old
        let fresh = drop_stale(&module, now_ms);
        assert_eq!(fresh.into_iter().map(|i| i.id).collect::<Vec<_>>(), vec!["fresh"]);
        assert_eq!(
            read(&module).into_iter().map(|i| i.id).collect::<Vec<_>>(),
            vec!["fresh"],
            "the trim must persist"
        );
    }

    #[test]
    fn remove_drops_only_the_named_ids() {
        let module = scratch("remove");
        enqueue(&module, item("a", 1));
        enqueue(&module, item("b", 2));
        enqueue(&module, item("c", 3));
        remove(&module, &["a".to_string(), "c".to_string()].into_iter().collect());
        assert_eq!(
            read(&module).into_iter().map(|i| i.id).collect::<Vec<_>>(),
            vec!["b"]
        );
    }

    #[test]
    fn reading_a_missing_or_corrupt_file_is_empty_not_a_panic() {
        let module = scratch("corrupt");
        assert!(read(&module).is_empty());
        module.ensure_dir().expect("mkdir");
        module.write_raw(QUEUE_FILE, b"not json").expect("write");
        assert!(read(&module).is_empty());
    }
}
