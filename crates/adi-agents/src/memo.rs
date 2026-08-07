//! Memoized reads of the files a run leaves behind.
//!
//! A session's transcript and its engine log are append-only, and both are re-read on **every
//! poll**: the open chat asks twice a second, and a settled turn's answer never changes again. Left
//! alone that means deserializing the same megabytes over and over — parsing a large event log with
//! `serde_json` costs hundreds of milliseconds, which is most of what a `peek` used to spend.
//!
//! The log side is keyed on bytes, not on a backend: a runner hands in the fold, so nothing here
//! knows one engine's wire format from another's (see [`folded_events`]).
//!
//! So each file's parsed form is kept, keyed by the file's identity on disk ([`Stamp`]: length plus
//! mtime). Both files only ever grow, so any change moves the length; the mtime is carried too, for
//! the one case where it does not — a log recreated by the next turn. A stamp that still matches
//! means the bytes are the ones we already parsed, and the parse is skipped entirely.
//!
//! Nothing here is a correctness boundary: a miss re-reads, and the worst a stale answer could do
//! is show a poll-old chat. The cache is bounded ([`CAPACITY`]) and evicts least-recently-used, so
//! a long-lived server watching many conversations keeps only the ones actually being read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use crate::progress::TurnContent;

/// How many parsed files are kept. Generous next to the handful of conversations a person watches
/// at once, and small enough that the whole cache is a rounding error beside one parsed log.
const CAPACITY: usize = 64;


/// A file's identity: two cheap `stat` fields that together change whenever its bytes do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: SystemTime,
}

impl Stamp {
    /// The stamp of `path` right now, or `None` when it cannot be stat'd (it does not exist yet,
    /// or the platform reports no mtime) — which reads as "not cacheable", never as "unchanged".
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            len: meta.len(),
            modified: meta.modified().ok()?,
        })
    }
}

/// One remembered parse, plus when it was last handed out (for eviction).
#[derive(Debug)]
struct Entry<T> {
    stamp: Stamp,
    value: Arc<T>,
    used: u64,
}

/// A bounded map from file path to its parsed contents.
#[derive(Debug)]
struct Memo<T> {
    entries: Mutex<HashMap<PathBuf, Entry<T>>>,
    /// Monotonic "logical clock" stamped onto every hit, so eviction can order entries by use.
    tick: AtomicU64,
    /// How many entries to keep. Per-map rather than one constant, because what an entry *costs*
    /// differs by orders of magnitude between a parsed transcript and a single number.
    capacity: usize,
}

impl<T> Memo<T> {
    fn new(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            tick: AtomicU64::new(0),
            capacity,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, Entry<T>>> {
        // A previous panic while holding this lock says nothing about the files on disk, so a
        // poisoned cache is taken anyway rather than propagating into every later read.
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The same, filed under `key` rather than under the file it was read from.
    ///
    /// For a reader whose answer depends on something besides the bytes — whether the writer has
    /// exited, say, which decides if a trailing half-line may be taken. Those are two different
    /// parses of one unchanged file, and filing both under the file would serve one where the other
    /// was asked for.
    fn get_or_insert_as(&self, key: PathBuf, path: &Path, parse: impl FnOnce() -> T) -> Arc<T> {
        let Some(stamp) = Stamp::of(path) else {
            // No file to key on — parse whatever the caller makes of that, but remember nothing.
            return Arc::new(parse());
        };
        let tick = self.tick.fetch_add(1, Ordering::Relaxed);
        {
            let mut entries = self.lock();
            if let Some(entry) = entries.get_mut(&key)
                && entry.stamp == stamp
            {
                entry.used = tick;
                return Arc::clone(&entry.value);
            }
        }

        let value = Arc::new(parse());
        let mut entries = self.lock();
        entries.insert(
            key,
            Entry {
                stamp,
                value: Arc::clone(&value),
                used: tick,
            },
        );
        evict(&mut entries, self.capacity);
        value
    }
}

/// Keep the cache bounded by dropping the least recently used entries once it is over capacity.
/// Halving rather than trimming to exactly `capacity` keeps this from running on every insert.
fn evict<T>(entries: &mut HashMap<PathBuf, Entry<T>>, capacity: usize) {
    if entries.len() <= capacity {
        return;
    }
    let mut ticks: Vec<u64> = entries.values().map(|e| e.used).collect();
    ticks.sort_unstable();
    let cutoff = ticks[ticks.len() / 2];
    entries.retain(|_, e| e.used >= cutoff);
}

/// The same content, arrived at through a runner's event stream. Its own map rather than a shared
/// one: two parsers keyed on the same path would each serve the other's answer.
static EVENTS: LazyLock<Memo<TurnContent>> = LazyLock::new(|| Memo::new(CAPACITY));



/// A turn's events, folded into content by the caller — memoized on the log they were read from.
///
/// Same bargain as [`parsed_log`], for the runner-driven path: a runner turns bytes into events on
/// every call, and an open chat asks twice a second. The log is the thing that changes, so it is the
/// thing keyed on; `fold` runs only when it has.
///
/// `complete` is part of the key rather than a hint. A runner reads only whole lines while the
/// writer is still going and takes the remainder once it has exited, so the same unchanged bytes
/// have two answers — and a child that exits without a trailing newline would otherwise settle
/// under the running one, committing an answer with its last line missing.
pub(crate) fn folded_events(
    path: &Path,
    complete: bool,
    fold: impl FnOnce() -> TurnContent,
) -> Arc<TurnContent> {
    let key = if complete {
        path.to_path_buf()
    } else {
        path.with_extension("log.partial")
    };
    EVENTS.get_or_insert_as(key, path, fold)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("adi-memo-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The point of the whole module: an unchanged file is parsed once, no matter how often it is
    /// asked for — and a file that grows is parsed again.
    #[test]
    fn a_file_is_reparsed_only_when_it_changes() {
        let dir = scratch("reparse");
        let path = dir.join("log");
        std::fs::write(&path, "one").unwrap();

        let memo: Memo<String> = Memo::new(CAPACITY);
        let parses = std::cell::Cell::new(0);
        let read = || {
            memo.get_or_insert_as(path.clone(), &path, || {
                parses.set(parses.get() + 1);
                std::fs::read_to_string(&path).unwrap_or_default()
            })
        };

        assert_eq!(*read(), "one");
        assert_eq!(*read(), "one");
        assert_eq!(parses.get(), 1, "the second read is served from the memo");

        // Appending moves the length, so the stamp no longer matches.
        std::fs::write(&path, "one two").unwrap();
        assert_eq!(*read(), "one two");
        assert_eq!(parses.get(), 2, "a changed file is parsed again");

        let _ = std::fs::remove_dir_all(dir);
    }

    /// A file that isn't there has no stamp to key on: it must still answer, and must not be
    /// remembered as if it had been read.
    #[test]
    fn a_missing_file_is_answered_but_not_cached() {
        let memo: Memo<String> = Memo::new(CAPACITY);
        let missing = std::env::temp_dir().join("adi-memo-nothing-here");
        let _ = std::fs::remove_file(&missing);

        assert_eq!(*memo.get_or_insert_as(missing.clone(), &missing, String::new), "");
        assert!(memo.lock().is_empty(), "nothing was remembered");
    }

    /// The cache is bounded, and what survives eviction is what has been read most recently.
    #[test]
    fn eviction_keeps_the_recently_used_and_bounds_the_cache() {
        let dir = scratch("evict");
        let memo: Memo<String> = Memo::new(CAPACITY);
        let mut paths = Vec::new();
        for i in 0..=CAPACITY {
            let path = dir.join(format!("f{i}"));
            std::fs::write(&path, format!("{i}")).unwrap();
            memo.get_or_insert_as(path.clone(), &path, || format!("{i}"));
            paths.push(path);
        }

        let kept = memo.lock().len();
        assert!(kept <= CAPACITY, "the cache stays bounded: {kept}");
        assert!(
            memo.lock().contains_key(paths.last().unwrap()),
            "the newest read survives"
        );
        assert!(
            !memo.lock().contains_key(&paths[0]),
            "the oldest read is the one dropped"
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
