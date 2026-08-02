//! What is waiting to be said next: `<id>.queue.json`, a plain list of messages beside the log.
//!
//! One turn runs at a time, so anything said while a session is still answering waits here. On disk
//! rather than in the browser, and for the same reason the record is: you can queue three things,
//! close the tab, and come back to find them answered.
//!
//! Nothing here knows whether a turn is running — that is the runner's question. The store only
//! keeps the line and hands out its head when asked.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::error::Result;

/// Serializes the read-modify-write pairs below.
///
/// Every operation here is load-edit-save on one file, and the callers are concurrent by
/// construction: an app server answering requests in parallel, an open chat polling twice a second,
/// and a CLI in another process entirely. Two ungated pairs interleaving would write back an entry
/// that had just been taken — a message asked twice, or one silently dropped. Queue edits are rare
/// and take microseconds, so one global gate costs nothing; plain reads never take it.
///
/// It is deliberately *not* a per-session lock: the cross-process case is not covered by any
/// in-process lock anyway, and a map of them would be more machinery for the same guarantee.
static QUEUE_GATE: Mutex<()> = Mutex::new(());

/// Hold the gate. A previous panic while holding it says nothing about the file on disk, so a
/// poisoned lock is taken anyway rather than propagating that panic into every later message.
fn gate() -> MutexGuard<'static, ()> {
    QUEUE_GATE.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) fn path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.queue.json"))
}

/// The messages waiting their turn, oldest first. An absent or unreadable file is an empty queue —
/// a queue is a convenience, never something worth failing a read over.
pub(super) fn load(dir: &Path, id: &str) -> Vec<String> {
    std::fs::read_to_string(path(dir, id))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .unwrap_or_default()
}

/// Write the queue back, removing the file once it empties — so "is anything waiting?", the question
/// asked on every poll, stays a single `exists`.
fn save(dir: &Path, id: &str, queue: &[String]) -> Result<()> {
    let file = path(dir, id);
    if queue.is_empty() {
        // A queue that was already empty leaves no file to remove; that is not a failure.
        if file.exists() {
            std::fs::remove_file(&file)?;
        }
        return Ok(());
    }
    let text = serde_json::to_string(queue).unwrap_or_else(|_| "[]".to_string());
    std::fs::write(&file, text)?;
    Ok(())
}

/// Put `message` at the back of the line, returning its **1-based** place in it.
///
/// One-based because the number is shown to the person who typed it: the message just queued is
/// "1st in line", and an empty queue that now holds one entry answers `1`.
///
/// # Errors
/// Returns the write error. A message that could not be written down was not queued, and the sender
/// has to hear that rather than wait for an answer that will never come.
pub(super) fn enqueue(dir: &Path, id: &str, message: &str) -> Result<usize> {
    let _gate = gate();
    let mut queue = load(dir, id);
    queue.push(message.to_string());
    let place = queue.len();
    save(dir, id, &queue)?;
    Ok(place)
}

/// Take the head of the line, or `None` when nothing is waiting.
///
/// The pop is persisted *before* the message is handed back, so a message that fails to launch has
/// still had its turn. Leaving it at the head would retry it on every poll for ever.
///
/// # Errors
/// Returns the write error, with the message left in the queue — the caller must not start a turn
/// whose removal was not recorded, or the same thing is asked twice.
pub(super) fn dequeue(dir: &Path, id: &str) -> Result<Option<String>> {
    let _gate = gate();
    let mut queue = load(dir, id);
    if queue.is_empty() {
        return Ok(None);
    }
    let head = queue.remove(0);
    save(dir, id, &queue)?;
    Ok(Some(head))
}

/// Drop the message at `index`, for something you have thought better of. Returns whether there was
/// one there to drop — an index past the end is a stale click, not an error.
///
/// # Errors
/// Returns the write error.
pub(super) fn unqueue(dir: &Path, id: &str, index: usize) -> Result<bool> {
    let _gate = gate();
    let mut queue = load(dir, id);
    if index >= queue.len() {
        return Ok(false);
    }
    queue.remove(index);
    save(dir, id, &queue)?;
    Ok(true)
}

/// Forget everything waiting — what stopping the current answer does. A queued message was written
/// expecting the answer you just cut short, so it goes with it.
///
/// # Errors
/// Returns the write error.
pub(super) fn clear(dir: &Path, id: &str) -> Result<()> {
    let _gate = gate();
    save(dir, id, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-store-queue-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn messages_are_answered_in_the_order_they_were_typed() {
        let dir = scratch("order");
        let id = "0000000000001-0000";
        assert!(load(&dir, id).is_empty(), "no file is an empty queue");
        assert!(dequeue(&dir, id).expect("dequeue").is_none());

        assert_eq!(enqueue(&dir, id, "first").expect("enqueue"), 1);
        assert_eq!(enqueue(&dir, id, "second").expect("enqueue"), 2);
        assert_eq!(enqueue(&dir, id, "third").expect("enqueue"), 3);
        assert_eq!(load(&dir, id), ["first", "second", "third"]);

        assert_eq!(dequeue(&dir, id).expect("dequeue").as_deref(), Some("first"));
        assert_eq!(load(&dir, id), ["second", "third"], "the pop is persisted");

        assert!(unqueue(&dir, id, 0).expect("unqueue"));
        assert_eq!(load(&dir, id), ["third"]);
        assert!(
            !unqueue(&dir, id, 5).expect("unqueue past the end"),
            "an index past the end changes nothing",
        );
        assert_eq!(load(&dir, id), ["third"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An emptied queue leaves no file behind, whichever way it emptied — that is what keeps the
    /// common poll ("anything waiting?") one `exists` rather than a read and a parse.
    #[test]
    fn an_emptied_queue_leaves_no_file_behind() {
        let dir = scratch("empty");
        let id = "0000000000002-0000";

        enqueue(&dir, id, "only one").expect("enqueue");
        assert!(path(&dir, id).exists());
        dequeue(&dir, id).expect("dequeue");
        assert!(!path(&dir, id).exists(), "drained by dequeue");

        enqueue(&dir, id, "again").expect("enqueue");
        enqueue(&dir, id, "and again").expect("enqueue");
        clear(&dir, id).expect("clear");
        assert!(!path(&dir, id).exists(), "dropped by clear");
        assert!(load(&dir, id).is_empty());
        // Clearing what is already clear is quiet, not an error about a missing file.
        clear(&dir, id).expect("clear again");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A corrupt queue file reads as empty rather than poisoning every later read: the messages in
    /// it are lost either way, and refusing to open the session on top of that helps nobody.
    #[test]
    fn a_corrupt_queue_reads_as_empty_and_recovers_on_the_next_write() {
        let dir = scratch("corrupt");
        let id = "0000000000003-0000";
        std::fs::write(path(&dir, id), "half a jso").unwrap();
        assert!(load(&dir, id).is_empty());
        assert_eq!(enqueue(&dir, id, "fresh").expect("enqueue"), 1);
        assert_eq!(load(&dir, id), ["fresh"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
