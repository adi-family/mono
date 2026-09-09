//! The two files a **split install** talks over, when routing and supervising are two processes.
//!
//! One hive owns the sockets a visitor arrives on; another owns the processes those visits are
//! meant to start. On this machine that is a route-only front door on `:80` (from `dns/`) and a
//! per-user supervisor (from `dashboards/`) — two `adi-hive`s started from two different config
//! files, with the on-demand registry ([`crate::demand`]) living in each one's memory. A visit
//! could therefore never wake anything: the process that saw it had no runner to start, and the
//! process with the runner never heard about the visit.
//!
//! So the two halves are joined by two small JSON files in the **store**, not beside either
//! config — the configs live in different directories, and "beside mine" is exactly what neither
//! process can resolve for the other:
//!
//! * [`wake_path`] — what the hive that *routes* wants: per route, a request that should start the
//!   service, and the stamp of the last request that landed on it. Written by the router, read by
//!   the supervisor.
//! * [`state_path`] — what the hive that *supervises* is doing: per service, its
//!   [phase](crate::demand::Phase). Written by the supervisor, read by the control panel (which
//!   has always read it here) **and** now by the router, which needs to know which services are
//!   supervised on demand before it asks for anything.
//!
//! Neither file is a lock or a queue. A request that fails to cross costs one refresh of the
//! holding page, which is asking again two seconds later; the design leans on that everywhere
//! rather than on delivery guarantees a pair of files cannot make.
//!
//! **Both hives have to be running a binary that knows about these files.** An old front door
//! writes no wake requests, and an old supervisor publishes its phases beside its own config where
//! nothing looks for them — in either case the visitor gets the pre-on-demand behaviour (a `502`
//! for a service that is down), never a wrong answer.
//!
//! **Keys are routes, not service names.** A `(host, path-prefix)` pair is what a request actually
//! lands on and is unique across the whole routing table by construction, while a service key like
//! `docs` repeats across imported projects — and a host on its own is not enough either, since two
//! services share one host when a dashboard's `/api` backend sits under its frontend. Waking the
//! frontend because somebody called the API would be the wrong service. So the key is the host,
//! plus the prefix when the route claims one: `nosh.adi`, `nosh.adi/api`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::warn;

/// What the supervising hive publishes about its on-demand services. The name is the panel's: it
/// has read `hive/demand.json` since on-demand services existed.
const STATE_FILE: &str = "demand.json";

/// What the routing hive asks of whoever supervises the service.
const WAKE_FILE: &str = "wake.json";

/// How long a wake request stays actionable. A request older than this is somebody who has long
/// since closed the tab — acting on it would start a service nobody is waiting for, which is the
/// one thing an on-demand policy must never do. Generous rather than tight: the supervisor may be
/// mid-restart when the request lands, and the visitor is still there a few seconds later.
const WAKE_FRESH: Duration = Duration::from_secs(10);

/// How often a route that is being *used* re-stamps its activity. Every request updates the
/// in-memory entry; only one every five seconds reaches the disk, so a page of fifty assets is one
/// write and a busy host is twelve a minute. The supervisor judges idleness in minutes, so five
/// seconds of lag in what it reads costs nothing.
const ACTIVITY_REFRESH: Duration = Duration::from_secs(5);

/// How long an entry survives without being re-stamped. This is what keeps the file from
/// accumulating: a host that stopped existing (a project removed, a `proxy.host` renamed) is
/// nobody's to delete, so it is dropped by whoever writes next instead — and five minutes is far
/// longer than any consumer needs, since the supervisor carries what it reads into its own clock
/// the moment it reads it.
const ENTRY_TTL: Duration = Duration::from_mins(5);

/// Where the supervisor publishes its phases: `~/.adi/mono/hive/demand.json`.
#[must_use]
pub fn state_path() -> PathBuf {
    crate::config::store_path(STATE_FILE)
}

/// Where the router leaves its wake requests: `~/.adi/mono/hive/wake.json`.
#[must_use]
pub fn wake_path() -> PathBuf {
    crate::config::store_path(WAKE_FILE)
}

/// One route's line in the wake file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Unix milliseconds of a request that should *start* this route's service. Set only when the
    /// router has reason to think it is not up — the last phase it read, or an upstream that just
    /// refused the connection — so a running service is never asked to start again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<u64>,
    /// Unix milliseconds of the most recent request routed here. This is the half the idle stop
    /// needs: without it a supervisor that routes nothing sees no traffic at all and stops a
    /// service somebody is using.
    pub activity: u64,
}

/// The writing half of the wake file, held by a hive that routes.
///
/// Keeps its own entries in memory and rewrites all of them on every flush. That is what makes two
/// writers over one file safe *enough*: a merge that loses an entry to a concurrent replace gets it
/// back on the next flush, without either process having to coordinate with the other.
#[derive(Debug)]
pub struct Wake {
    path: PathBuf,
    mine: Mutex<Mine>,
}

/// What this process has asked for, and when it last said so out loud.
#[derive(Debug, Default)]
struct Mine {
    entries: BTreeMap<String, Entry>,
    flushed: Option<Instant>,
}

impl Wake {
    /// A writer for the wake file at `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            mine: Mutex::new(Mine::default()),
        }
    }

    /// Say that a request just landed on `route`, and — when `wake` — that the service behind it
    /// needs starting.
    ///
    /// A wake reaches the disk immediately: somebody is on a holding page waiting for it. A plain
    /// activity stamp waits for the next flush, at most [`ACTIVITY_REFRESH`] away.
    pub fn stamp(&self, route: &str, wake: bool) {
        let now = unix_ms();
        let mut mine = self.lock();
        let entry = mine.entries.entry(route.to_string()).or_default();
        // A route we have never stamped may well be one whose service is down, so the first
        // request for it is worth a write whatever else it is.
        let first = entry.activity == 0;
        entry.activity = now;
        if wake {
            entry.wake = Some(now);
        }
        let due = wake
            || first
            || mine
                .flushed
                .is_none_or(|last| last.elapsed() >= ACTIVITY_REFRESH);
        if !due {
            return;
        }
        mine.flushed = Some(Instant::now());
        mine.entries.retain(|_, e| fresh(e.activity, now));
        let merged = merge(&self.path, &mine.entries, now);
        let written = serde_json::to_vec_pretty(&merged)
            .map_err(std::io::Error::other)
            .and_then(|json| write(&self.path, &json));
        if let Err(e) = written {
            // Not fatal, and deliberately not silent: this is the whole channel, and the usual
            // cause is a store directory a *different* user created (see [`write`]).
            warn!(error = %e, path = %self.path.display(), "could not write the wake file");
        }
    }

    fn lock(&self) -> MutexGuard<'_, Mine> {
        self.mine.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Everything the file should say once this process has had its turn: whatever else is in it that
/// is still fresh — another router's entries — with ours laid over the top.
fn merge(path: &Path, mine: &BTreeMap<String, Entry>, now: u64) -> BTreeMap<String, Entry> {
    let mut merged = read(path);
    merged.retain(|_, e| fresh(e.activity, now));
    for (route, entry) in mine {
        merged.insert(route.clone(), *entry);
    }
    merged
}

/// Whether a stamp is recent enough to keep. A stamp from the future — a clock that moved
/// backwards, or a file written on another machine's clock through a synced home directory — is
/// kept rather than dropped, since the only harm it can do is expire late.
fn fresh(stamp: u64, now: u64) -> bool {
    age(stamp, now) < ENTRY_TTL
}

/// How long ago a unix-millisecond stamp was, from `now` in the same units.
fn age(stamp: u64, now: u64) -> Duration {
    Duration::from_millis(now.saturating_sub(stamp))
}

/// Read the wake file. A missing, unreadable or unparsable file is *no entries* — there is nothing
/// a router or a supervisor could usefully do about one, and both would rather keep serving.
#[must_use]
pub fn read(path: &Path) -> BTreeMap<String, Entry> {
    std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

/// Read the published phases: service name → phase, exactly as [`crate::demand`] writes them.
/// Same tolerance as [`read`] — an absent file means nobody is supervising anything on demand.
#[must_use]
pub fn read_phases(path: &Path) -> BTreeMap<String, String> {
    std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

/// Replace `path` with `bytes`: a fresh temp file in the same directory, then a rename.
///
/// Atomic because both files have a reader in another process, polling: a `write(2)` that
/// truncates first is a window in which the other hive reads half a file, and it would read it as
/// an empty one. The result is `0644` from the moment it exists, for the same reason
/// `hive/status.json` is — on macOS the front door is a **root** daemon and the supervisor and the
/// panel that read it are not (see `adi-config`'s `fsutil`, "What is deliberately not private").
///
/// It deliberately does **not** create the directory. Whoever creates `hive/` owns it, and a root
/// front door that got there first would own a directory the per-user supervisor then could not
/// publish into. The supervisor creates it (see [`crate::demand::Demand::publish`]), and until it
/// has there is nothing to wake anyway — the router only stamps routes it found in the file the
/// supervisor writes *into that directory*.
///
/// # Errors
/// The temp file could not be written or renamed — a missing or unwritable store directory.
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let name = path.file_name().map_or_else(
        || "shared".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let tmp = path.with_file_name(format!("{name}.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o644);
    }
    let write_tmp = options
        .open(&tmp)
        .and_then(|mut file| file.write_all(bytes))
        .and_then(|()| std::fs::rename(&tmp, path));
    if write_tmp.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_tmp
}

/// The reading half of the wake file, held by a hive that supervises.
///
/// It is the reader, not the file, that remembers which requests have been answered: a wake is
/// acted on once, by the watermark below, so nothing in the file can start a service twice — and a
/// supervisor need never write to a file another process is writing.
#[derive(Debug)]
pub struct Reader {
    path: PathBuf,
    /// The newest wake this reader has already acted on, per route.
    acted: BTreeMap<String, u64>,
}

/// What one route's entry is asking for, once the reader has decided what is new in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// The route key — a host, plus a path prefix when the route claims one.
    pub route: String,
    /// Start the service: a request has arrived that this reader has not answered yet, recent
    /// enough that somebody is still waiting for it.
    pub start: bool,
    /// How long ago the newest request for this route was, for the supervisor's idle clock.
    pub idle_for: Duration,
}

impl Reader {
    /// A reader of the wake file at `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            acted: BTreeMap::new(),
        }
    }

    /// Everything the file is asking for right now.
    ///
    /// Re-reads on every call rather than trusting a modification time: the file is a few hundred
    /// bytes, this runs a few times a second, and a filesystem whose timestamps are coarser than
    /// the poll would silently swallow the one wake somebody is waiting on.
    pub fn poll(&mut self) -> Vec<Pending> {
        let entries = read(&self.path);
        let now = unix_ms();
        let pending = entries
            .iter()
            .map(|(route, entry)| {
                let acted = self.acted.get(route).copied().unwrap_or(0);
                let start = entry
                    .wake
                    .is_some_and(|at| at > acted && age(at, now) <= WAKE_FRESH);
                Pending {
                    route: route.clone(),
                    start,
                    idle_for: age(entry.activity, now),
                }
            })
            .collect();
        for (route, entry) in &entries {
            if let Some(at) = entry.wake {
                let acted = self.acted.entry(route.clone()).or_default();
                *acted = (*acted).max(at);
            }
        }
        // A route that has left the file takes its watermark with it, so the map cannot outgrow
        // the routing table it describes.
        self.acted.retain(|route, _| entries.contains_key(route));
        pending
    }
}

/// Now, in unix milliseconds — the one clock two processes can compare. `Instant` cannot cross a
/// process boundary, so every stamp in these files is wall-clock and every reader turns it back
/// into an age before using it.
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-hive-shared-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch dir");
        dir
    }

    /// The channel end to end, at the level of the file itself: what one process stamps is what the
    /// other reads, and a request is answered exactly once.
    #[test]
    fn a_wake_is_read_once_and_activity_goes_on_being_read() {
        let dir = scratch("once");
        let path = dir.join("wake.json");
        let wake = Wake::new(path.clone());
        let mut reader = Reader::new(path.clone());

        assert!(reader.poll().is_empty(), "nothing has asked for anything");

        wake.stamp("web.adi", true);
        let pending = reader.poll();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].route, "web.adi");
        assert!(pending[0].start, "the request has not been answered yet");
        assert!(pending[0].idle_for < Duration::from_secs(1));

        // The same request must not start the service a second time — but the activity it carries
        // is still the truth about when this route was last used.
        let pending = reader.poll();
        assert_eq!(pending.len(), 1);
        assert!(!pending[0].start, "one request is one start");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A wake nobody is waiting for any more must not start anything: the visitor left, and an
    /// on-demand service that starts itself for a closed tab is the cost the policy exists to
    /// avoid. The same file's activity stamp is still read, and still says an hour ago.
    #[test]
    fn a_stale_wake_never_starts_a_service() {
        let dir = scratch("stale");
        let path = dir.join("wake.json");
        let old = unix_ms() - 60 * 60 * 1000;
        let entries = BTreeMap::from([(
            "web.adi".to_string(),
            Entry {
                wake: Some(old),
                activity: old,
            },
        )]);
        write(&path, &serde_json::to_vec(&entries).expect("json")).expect("seed");

        let pending = Reader::new(path).poll();
        assert_eq!(pending.len(), 1);
        assert!(!pending[0].start, "an hour-old request wakes nothing");
        assert!(pending[0].idle_for >= Duration::from_secs(3599));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two hives may write this file — a machine can have more than one front door — so a write
    /// must not be a way of deleting what the other one asked for.
    #[test]
    fn two_writers_keep_each_others_entries() {
        let dir = scratch("writers");
        let path = dir.join("wake.json");
        let (first, second) = (Wake::new(path.clone()), Wake::new(path.clone()));

        first.stamp("one.adi", true);
        second.stamp("two.adi", true);
        let entries = read(&path);
        assert_eq!(entries.len(), 2, "{entries:?}");

        // And a writer that flushes again still carries the other's entry through the merge.
        first.stamp("one.adi", true);
        let entries = read(&path);
        assert_eq!(entries.len(), 2, "{entries:?}");
        assert!(entries["two.adi"].wake.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An entry for a route that no longer exists is dropped by whoever writes next, rather than
    /// sitting in the file for ever — nothing else is ever going to remove it.
    #[test]
    fn a_route_that_stopped_being_stamped_ages_out_of_the_file() {
        let dir = scratch("ttl");
        let path = dir.join("wake.json");
        let stale = u64::try_from(ENTRY_TTL.as_millis()).expect("the TTL fits a u64") + 1_000;
        let entries = BTreeMap::from([
            (
                "gone.adi".to_string(),
                Entry {
                    wake: None,
                    activity: unix_ms() - stale,
                },
            ),
            (
                "recent.adi".to_string(),
                Entry {
                    wake: None,
                    activity: unix_ms(),
                },
            ),
        ]);
        write(&path, &serde_json::to_vec(&entries).expect("json")).expect("seed");

        Wake::new(path.clone()).stamp("live.adi", true);
        let entries = read(&path);
        assert!(!entries.contains_key("gone.adi"), "{entries:?}");
        assert!(entries.contains_key("recent.adi"), "{entries:?}");
        assert!(entries.contains_key("live.adi"), "{entries:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Activity is throttled and a wake is not: a page of assets costs one write, and the request
    /// that needs a service started never waits for a timer.
    #[test]
    fn activity_is_throttled_but_a_wake_is_written_at_once() {
        let dir = scratch("throttle");
        let path = dir.join("wake.json");
        let wake = Wake::new(path.clone());

        wake.stamp("web.adi", false);
        let first = read(&path)["web.adi"].activity;
        assert!(first > 0, "the first request for a route is always written");

        std::thread::sleep(Duration::from_millis(20));
        wake.stamp("web.adi", false);
        assert_eq!(
            read(&path)["web.adi"].activity,
            first,
            "a second request inside the refresh window costs no write"
        );

        std::thread::sleep(Duration::from_millis(20));
        wake.stamp("web.adi", true);
        let entry = read(&path)["web.adi"];
        assert!(entry.activity > first, "a wake flushes immediately");
        assert!(entry.wake.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The file is replaced, never truncated in place: a reader mid-poll sees the old file or the
    /// new one. Checked through the mode as well, since a root front door writes what an
    /// unprivileged supervisor has to read.
    #[test]
    fn a_write_replaces_the_file_and_leaves_it_readable() {
        let dir = scratch("write");
        let path = dir.join("wake.json");
        write(&path, b"{}").expect("write");
        write(&path, b"{\"a\":{\"activity\":1}}").expect("replace");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "{\"a\":{\"activity\":1}}"
        );
        assert!(
            std::fs::read_dir(&dir)
                .expect("dir")
                .flatten()
                .all(|e| e.file_name() == "wake.json"),
            "the temp file must not be left behind"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o644, "0o{:o}", mode & 0o777);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A store directory that is not there is not a reason to stop serving — and it is what a
    /// machine where no hive supervises anything on demand looks like.
    #[test]
    fn writing_where_there_is_no_directory_fails_without_creating_one() {
        let dir = scratch("nodir");
        let missing = dir.join("hive/wake.json");
        assert!(write(&missing, b"{}").is_err());
        assert!(
            !dir.join("hive").exists(),
            "the writer creates no directory"
        );
        Wake::new(missing).stamp("web.adi", true);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
