//! Memoized reads of the files a run leaves behind.
//!
//! A conversation's transcript and its engine log are append-only, and both are re-read on **every
//! poll**: the open chat asks twice a second, and a settled turn's answer never changes again. Left
//! alone that means deserializing the same megabytes over and over — parsing a large event log with
//! `serde_json` costs hundreds of milliseconds, which is most of what a `peek` used to spend.
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

use crate::backend::Backend;
use crate::progress::{self, MAX_PARSE_BYTES, TurnContent};
use crate::run::Turn;

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
}

impl<T> Memo<T> {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            tick: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, Entry<T>>> {
        // A previous panic while holding this lock says nothing about the files on disk, so a
        // poisoned cache is taken anyway rather than propagating into every later read.
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The parsed form of `path`, reading and re-parsing it only when its stamp has moved.
    ///
    /// `parse` is deliberately run **outside** the lock: it is the slow part, and holding the cache
    /// across it would make every other conversation's poll wait on this one. Two callers racing on
    /// the same cold file both parse it, which is merely the work they would each have done anyway.
    fn get_or_insert(&self, path: &Path, parse: impl FnOnce() -> T) -> Arc<T> {
        let Some(stamp) = Stamp::of(path) else {
            // No file to key on — parse whatever the caller makes of that, but remember nothing.
            return Arc::new(parse());
        };
        let tick = self.tick.fetch_add(1, Ordering::Relaxed);
        {
            let mut entries = self.lock();
            if let Some(entry) = entries.get_mut(path)
                && entry.stamp == stamp
            {
                entry.used = tick;
                return Arc::clone(&entry.value);
            }
        }

        let value = Arc::new(parse());
        let mut entries = self.lock();
        entries.insert(
            path.to_path_buf(),
            Entry {
                stamp,
                value: Arc::clone(&value),
                used: tick,
            },
        );
        evict(&mut entries);
        value
    }
}

/// Keep the cache bounded by dropping the least recently used entries once it is over capacity.
/// Halving rather than trimming to exactly `CAPACITY` keeps this from running on every insert.
fn evict<T>(entries: &mut HashMap<PathBuf, Entry<T>>) {
    if entries.len() <= CAPACITY {
        return;
    }
    let mut ticks: Vec<u64> = entries.values().map(|e| e.used).collect();
    ticks.sort_unstable();
    let cutoff = ticks[ticks.len() / 2];
    entries.retain(|_, e| e.used >= cutoff);
}

/// Parsed engine logs — a run's captured output as [`TurnContent`].
static LOGS: LazyLock<Memo<TurnContent>> = LazyLock::new(Memo::new);

/// Parsed conversation transcripts — the committed turns of a `<conv_id>.jsonl`.
static TRANSCRIPTS: LazyLock<Memo<Vec<Turn>>> = LazyLock::new(Memo::new);

/// One run's captured output, parsed into its progress content. Re-parsed only when the log has
/// actually grown since the last read; a finished run is parsed exactly once, however often it is
/// polled afterwards.
pub(crate) fn parsed_log(backend: &Backend, path: &Path) -> Arc<TurnContent> {
    LOGS.get_or_insert(path, || progress::parse(backend, &read_capped(path)))
}

/// One conversation's committed turns, oldest first. Same deal: the transcript is re-read only
/// after a turn has been appended to it.
pub(crate) fn transcript(path: &Path) -> Arc<Vec<Turn>> {
    TRANSCRIPTS.get_or_insert(path, || {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Turn>(l).ok())
            .collect()
    })
}

/// Read a log file whole, from the start, up to the progress parse cap. Reading from the start (not
/// a tail) keeps the beginning of a streamed event log, so early tool steps survive.
fn read_capped(path: &Path) -> Vec<u8> {
    use std::io::Read as _;
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let _ = file.take(MAX_PARSE_BYTES).read_to_end(&mut buf);
    buf
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

        let memo: Memo<String> = Memo::new();
        let parses = std::cell::Cell::new(0);
        let read = || {
            memo.get_or_insert(&path, || {
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
        let memo: Memo<String> = Memo::new();
        let missing = std::env::temp_dir().join("adi-memo-nothing-here");
        let _ = std::fs::remove_file(&missing);

        assert_eq!(*memo.get_or_insert(&missing, String::new), "");
        assert!(memo.lock().is_empty(), "nothing was remembered");
    }

    /// The cache is bounded, and what survives eviction is what has been read most recently.
    #[test]
    fn eviction_keeps_the_recently_used_and_bounds_the_cache() {
        let dir = scratch("evict");
        let memo: Memo<String> = Memo::new();
        let mut paths = Vec::new();
        for i in 0..=CAPACITY {
            let path = dir.join(format!("f{i}"));
            std::fs::write(&path, format!("{i}")).unwrap();
            memo.get_or_insert(&path, || format!("{i}"));
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
