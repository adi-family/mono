//! Bringing the old layout forward: `<sessions>/<process|harness>/<agent>/<id>.*` → `<sessions>/<agent>/<id>.*`.
//!
//! There are real sessions on real machines under the old paths, and a refactor that reads a new
//! path is, from the outside, a refactor that deleted everybody's history. So the files move, once,
//! the first time the new store is opened.
//!
//! # What has to be rewritten, and what merely moves
//!
//! A session's own files (`.log`, `.pid`, `.queue.json`, and whatever a runner spooled beside them)
//! carry no path in them, so they are moved verbatim — including sidecars this module has never
//! heard of, which is the same reason [`delete`](super::SessionStore::delete) sweeps by prefix.
//!
//! Two names change, because the new layout gave them different ones:
//!
//! - `<id>.json` was the metadata sidecar; the record now lives in `<id>.meta.json` and holds
//!   different fields, so it is *rebuilt* rather than moved (and the old one dropped, since a
//!   stray `<id>.json` would be an orphan nothing reads).
//! - `<id>.jsonl` was a conversation's transcript, now `<id>.transcript.jsonl`. Moving it under its
//!   old name would leave every migrated conversation looking like it had never said anything.
//!
//! # The one field that cannot be recovered
//!
//! The old path said `process` or `harness` — the *executor*, not the engine. `process/claude` and
//! `process/codex` were the same directory, so which one a session ran under is simply not in the
//! tree. It comes from the agent's manifest instead, via the `backend_of` callback the caller
//! supplies: this module has no business reading manifests.
//!
//! # Idempotent, and never destructive
//!
//! A session already present in the new layout is left completely alone — its legacy copy is not
//! moved, not merged, and not deleted. Two ids can collide across executors (`process/a/<id>` and
//! `harness/a/<id>` are different sessions with the same name), and the surviving one must be the
//! one the new store has already been using. A legacy directory that still holds something is left
//! standing for the same reason; only the ones that emptied are removed. So a second run moves
//! nothing, and reports it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::Backend;
use crate::error::Result;

use super::{SessionRecord, SessionStore, exists_in, record, transcript};

/// The executor directories the old layout filed sessions under. `pty` is absent on purpose: a pty
/// session is a live pane, it keeps nothing on disk, and there was never a directory for it.
const LEGACY_DIRS: [&str; 2] = ["process", "harness"];

/// The old metadata sidecar, as it read after the session id — `<id>.json`, one dot shorter than the
/// `<id>.meta.json` it becomes.
const LEGACY_META_SUFFIX: &str = "json";

/// The old transcript sidecar: `<id>.jsonl`.
const LEGACY_TRANSCRIPT_SUFFIX: &str = "jsonl";

/// Where the old layout kept a running child's pid: `<id>.pid`, dropped by a reaper when the child
/// exited. Liveness is the runner's state slot now, so the number moves there — a run that is alive
/// while the store is opened must not read as finished the moment it is brought forward, leaving its
/// child burning tokens where nothing lists it.
const LEGACY_PID_SUFFIX: &str = "pid";

/// Move every session in the old layout into the new one. Returns how many were moved.
///
/// # Errors
/// Returns the directory-creation, move, and record-write errors. A failure part-way through leaves
/// the sessions it already moved moved — they are readable, and a later run finishes the rest.
pub(super) fn legacy(store: &SessionStore, backend_of: &dyn Fn(&str) -> Backend) -> Result<usize> {
    let mut moved = 0;
    for executor in LEGACY_DIRS {
        let root = store.dir().join(executor);
        // Nothing filed under that executor — the common case, and the only case after the first run.
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            // Directories only. If an agent is *named* `process`, this same path is its own session
            // directory in the new layout and holds `<id>.*` files rather than agent dirs; skipping
            // plain files means that agent's sessions are never mistaken for a legacy tree.
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Ok(agent) = entry.file_name().into_string() else {
                continue;
            };
            // The manifest is the only place the engine can come from — but an agent whose
            // definition has been deleted still has sessions, and one of them may still be
            // answering. Falling back to the executor keeps those managed rather than orphaning a
            // live child where nothing lists it: `process` and `harness` are one runner whichever
            // engine wrote them, which is all liveness, stop, and delete depend on.
            let backend = |agent: &str| match backend_of(agent) {
                Backend::Other(wire) if wire.trim().is_empty() => engine_of(executor),
                known => known,
            };
            moved += one_agent(store, &entry.path(), &agent, &backend)?;
            // Only if it emptied: anything left behind is a session the new layout already had.
            let _ = std::fs::remove_dir(entry.path());
        }
        let _ = std::fs::remove_dir(&root);
    }
    Ok(moved)
}

/// Move one legacy agent directory's sessions. Returns how many were moved.
fn one_agent(
    store: &SessionStore,
    from: &Path,
    agent: &str,
    backend_of: &dyn Fn(&str) -> Backend,
) -> Result<usize> {
    let sessions = sessions_in(from);
    if sessions.is_empty() {
        return Ok(0);
    }
    let to = store.agent_dir(agent);
    std::fs::create_dir_all(&to)?;

    let mut moved = 0;
    for (id, files) in sessions {
        if exists_in(&to, &id) {
            continue;
        }
        let mut meta = LegacyMeta::read(&from.join(format!("{id}.{LEGACY_META_SUFFIX}")));
        meta.pid = read_pid(&from.join(format!("{id}.{LEGACY_PID_SUFFIX}")));
        for name in &files {
            let Some((_, suffix)) = name.split_once('.') else {
                continue;
            };
            match suffix {
                // Rebuilt below, into a different file with different fields.
                LEGACY_META_SUFFIX => {}
                LEGACY_TRANSCRIPT_SUFFIX => {
                    move_file(&from.join(name), &transcript::path(&to, &id))?;
                }
                _ => move_file(&from.join(name), &to.join(name))?,
            }
        }
        // The record last: it is what makes the session *exist* in the new layout, so writing it
        // only once its files are in place means a run that dies half way is resumed rather than
        // skipped as already-migrated.
        record::write(&to, &meta.into_record(store, &id, agent, backend_of))?;
        let _ = std::fs::remove_file(from.join(format!("{id}.{LEGACY_META_SUFFIX}")));
        moved += 1;
    }
    Ok(moved)
}

/// The engine to assume for a session whose agent can no longer say. The old path named only the
/// executor, and both engines under one shared a directory, so this is a guess — but a guess within
/// the right runner, which is what makes the session manageable at all.
fn engine_of(executor: &str) -> Backend {
    match executor {
        "harness" => Backend::HarnessClaudeSdk,
        _ => Backend::ProcessClaude,
    }
}

/// Every session in a legacy agent directory, as id → the names of the files it owns.
///
/// Keyed off the `<id>.<rest>` shape alone rather than a list of known extensions: a session is
/// whatever shares a prefix, so a sidecar invented after this module was written still travels with
/// the session it belongs to.
fn sessions_in(dir: &Path) -> BTreeMap<String, Vec<String>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    let mut sessions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        // Session ids hold no dots, so everything before the first one is the id.
        let Some((id, _)) = name.split_once('.') else {
            continue;
        };
        sessions.entry(id.to_string()).or_default().push(name);
    }
    sessions
}

/// Move one file, falling back to copy-and-delete when a plain rename cannot do it.
///
/// Both paths are under the sessions root so a rename is all but certain, but a root spanning two
/// mounts (a bind-mounted or symlinked agent directory) would fail it — and losing a session's log
/// to a tidier implementation is not a trade worth making.
fn move_file(from: &Path, to: &Path) -> Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

/// The old metadata sidecar's fields — the ones that still mean something.
#[derive(Debug, Default)]
struct LegacyMeta {
    started_at: u64,
    message: String,
    hidden: bool,
    /// The engine session a harness conversation resumed. Runner-private now, so it moves into the
    /// state slot rather than staying a field of the record.
    session_id: String,
    /// The directory the session ran in, recorded so every later turn re-entered it. It is
    /// [`cwd`](SessionRecord::cwd) now, under a name that says what it is.
    working_dir: String,
    /// The child answering this session right now, from its `<id>.pid` file. Runner-private too:
    /// liveness is a question for the runner's state slot, not for a file the store knows about.
    pid: Option<u32>,
}

impl LegacyMeta {
    /// Read a legacy sidecar, field by field so one unexpected value does not lose the rest, and
    /// all-default when it is absent or unreadable (an old run predating the sidecar entirely).
    fn read(path: &Path) -> Self {
        let Some(value) = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        else {
            return Self::default();
        };
        let string = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        Self {
            started_at: value
                .get("started_at")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            message: string("message"),
            hidden: value
                .get("hidden")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            session_id: string("session_id"),
            working_dir: string("working_dir"),
            // Not in the sidecar — it was a file of its own, read by the caller.
            pid: None,
        }
    }

    /// The same session, said the new way.
    fn into_record(
        self,
        store: &SessionStore,
        id: &str,
        agent: &str,
        backend_of: &dyn Fn(&str) -> Backend,
    ) -> SessionRecord {
        let runner_state = self.runner_state();
        SessionRecord {
            id: id.to_string(),
            agent: agent.to_string(),
            // Not in the tree: the old path named the executor, and two engines shared each one.
            backend: backend_of(agent),
            cwd: if self.working_dir.is_empty() {
                // A session old enough to have recorded no directory ran wherever the launcher
                // defaulted to, which was the store root — the sessions directory's parent. Pinning
                // that is what keeps a resumed turn in the place its earlier turns wrote into.
                store.dir().parent().map(Path::to_path_buf).unwrap_or_default()
            } else {
                PathBuf::from(self.working_dir)
            },
            message: self.message,
            started_at: if self.started_at > 0 {
                self.started_at
            } else {
                record::started_at(id)
            },
            // Derived from the files on every read, never stored.
            last_activity: 0,
            hidden: self.hidden,
            runner_state,
        }
    }

    /// What the detached runner would have written for this session: the child it is watching, and
    /// the engine session that child established.
    ///
    /// Both are absent for most sessions — a finished run has no pid and only `harness:claude-sdk`
    /// keeps a session id — and an empty slot is better said by leaving it out than by writing
    /// `{"session_id": ""}`.
    fn runner_state(&self) -> Option<serde_json::Value> {
        let mut state = serde_json::Map::new();
        if let Some(pid) = self.pid {
            state.insert("pid".to_string(), serde_json::json!(pid));
        }
        if !self.session_id.is_empty() {
            state.insert(
                "session_id".to_string(),
                serde_json::Value::String(self.session_id.clone()),
            );
        }
        (!state.is_empty()).then_some(serde_json::Value::Object(state))
    }
}

/// The pid in a legacy `<id>.pid` file, when there is one to read.
fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> SessionStore {
        let dir = std::env::temp_dir().join(format!(
            "adi-store-migrate-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        SessionStore::new(dir)
    }

    /// Lay down one session the old way, under an executor directory.
    fn seed_legacy(store: &SessionStore, executor: &str, agent: &str, id: &str, meta: &str) -> PathBuf {
        let dir = store.dir().join(executor).join(agent);
        std::fs::create_dir_all(&dir).unwrap();
        if !meta.is_empty() {
            std::fs::write(dir.join(format!("{id}.json")), meta).unwrap();
        }
        std::fs::write(dir.join(format!("{id}.log")), "what it said").unwrap();
        dir
    }

    fn backends(agent: &str) -> Backend {
        match agent {
            "chat" => Backend::HarnessClaudeSdk,
            _ => Backend::ProcessCodex,
        }
    }

    /// The whole point: a session filed the old way is readable the new way afterwards — its record
    /// rebuilt, its transcript intact, its odd sidecars carried along — and running the migration
    /// again does nothing at all.
    #[test]
    fn a_legacy_session_lands_flat_with_its_record_rebuilt() {
        let store = scratch("move");
        let id = "0000000012345-0000";
        let dir = seed_legacy(
            &store,
            "harness",
            "chat",
            id,
            r#"{"started_at":12345,"message":"set the port","session_id":"sid-7","working_dir":"/tmp/work","hidden":true}"#,
        );
        // A conversation's transcript, its queue, and a sidecar this module has never heard of.
        std::fs::write(
            dir.join(format!("{id}.jsonl")),
            "{\"role\":\"user\",\"text\":\"set the port\",\"at\":12345}\n",
        )
        .unwrap();
        std::fs::write(dir.join(format!("{id}.queue.json")), r#"["and restart"]"#).unwrap();
        std::fs::write(dir.join(format!("{id}.pid")), "4711\n").unwrap();
        std::fs::write(dir.join(format!("{id}.invented-later")), "who knows").unwrap();

        assert_eq!(store.migrate_legacy(backends).expect("migrate"), 1);

        let session = store.get("chat", id).expect("found the new way");
        assert_eq!(session.backend, Backend::HarnessClaudeSdk, "from the manifest");
        assert_eq!(session.cwd, PathBuf::from("/tmp/work"), "working_dir became cwd");
        assert_eq!(session.message, "set the port");
        assert_eq!(session.started_at, 12_345);
        assert!(session.hidden, "a hidden session stays hidden");
        assert_eq!(
            session.runner_state,
            Some(serde_json::json!({ "pid": 4711, "session_id": "sid-7" })),
            "the child it is watching and the engine session id are both runner-private now — and \
             a run that is alive when the store is opened must not read as finished afterwards",
        );

        let flat = store.agent_dir("chat");
        assert_eq!(std::fs::read_to_string(store.log_path("chat", id)).unwrap(), "what it said");
        assert_eq!(store.queued("chat", id), ["and restart"]);
        assert_eq!(store.turns("chat", id).len(), 1, "the transcript came with it");
        assert_eq!(store.turns("chat", id)[0].text, "set the port");
        assert!(flat.join(format!("{id}.pid")).is_file());
        assert!(
            flat.join(format!("{id}.invented-later")).is_file(),
            "a sidecar nobody recognises still travels with its session",
        );
        assert!(
            !flat.join(format!("{id}.json")).exists(),
            "the old sidecar is not left behind as an orphan",
        );
        assert!(
            !store.dir().join("harness").exists(),
            "the emptied legacy tree is gone",
        );

        // Twice is once: nothing to move, nothing changed.
        assert_eq!(store.migrate_legacy(backends).expect("again"), 0);
        assert_eq!(store.get("chat", id).map(|s| s.message), Some("set the port".into()));
        assert_eq!(store.turns("chat", id).len(), 1);

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Both executor directories are swept, the backend of each session comes from its own agent,
    /// and a session too old to have recorded where it ran is pinned to the store root — the
    /// directory it actually ran in — rather than left to be re-resolved somewhere else later.
    #[test]
    fn every_executor_is_swept_and_a_directoryless_session_keeps_the_store_root() {
        let store = scratch("sweep");
        let old = "0000000000001-0000";
        let new = "0000000000002-0000";
        seed_legacy(&store, "process", "runner", old, "");
        seed_legacy(
            &store,
            "process",
            "runner",
            new,
            r#"{"started_at":2,"message":"a plain run"}"#,
        );
        seed_legacy(&store, "harness", "chat", old, r#"{"message":"a conversation"}"#);

        assert_eq!(store.migrate_legacy(backends).expect("migrate"), 3);

        let listed = store.list("runner");
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|s| s.backend == Backend::ProcessCodex));
        let root = store.dir().parent().expect("the sessions dir has a parent");
        assert!(
            listed.iter().all(|s| s.cwd == root),
            "a session with no recorded directory ran in the store root",
        );
        let no_meta = store.get("runner", old).expect("found");
        assert_eq!(no_meta.started_at, 1, "recovered from the id");
        assert_eq!(no_meta.message, "");
        assert!(no_meta.runner_state.is_none(), "no engine session to resume");

        // Same id under two executors: they are different sessions of different agents, and both
        // survive because the agent, not the executor, is now the only path segment.
        let conversation = store.get("chat", old).expect("found");
        assert_eq!(conversation.backend, Backend::HarnessClaudeSdk);
        assert_eq!(conversation.message, "a conversation");
        assert!(!store.dir().join("process").exists());
        assert!(!store.dir().join("harness").exists());

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The rule that makes a re-run safe even when the ids collide: what the new store already has
    /// is what it keeps. The legacy copy is left standing rather than merged or deleted — it is a
    /// different session that happens to share a name.
    #[test]
    fn a_session_that_already_exists_in_the_new_layout_is_never_clobbered() {
        let store = scratch("collide");
        let id = "0000000000009-0000";
        seed_legacy(&store, "harness", "chat", id, r#"{"message":"the old one"}"#);
        // The same id, already migrated (or simply created) in the new layout.
        let flat = store.agent_dir("chat");
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::write(store.log_path("chat", id), "the one in use").unwrap();
        record::write(
            &flat,
            &SessionRecord {
                id: id.to_string(),
                agent: "chat".into(),
                backend: Backend::HarnessAdi,
                cwd: PathBuf::from("/tmp/live"),
                message: "the one in use".into(),
                started_at: 9,
                last_activity: 0,
                hidden: false,
                runner_state: None,
            },
        )
        .unwrap();

        assert_eq!(store.migrate_legacy(backends).expect("migrate"), 0);

        let kept = store.get("chat", id).expect("still there");
        assert_eq!(kept.message, "the one in use", "the live session won");
        assert_eq!(kept.backend, Backend::HarnessAdi);
        assert_eq!(kept.cwd, PathBuf::from("/tmp/live"));
        assert_eq!(
            std::fs::read_to_string(store.log_path("chat", id)).unwrap(),
            "the one in use",
        );
        // And the legacy copy is still there, untouched, rather than half-merged or destroyed.
        let legacy = store.dir().join("harness").join("chat");
        assert_eq!(
            std::fs::read_to_string(legacy.join(format!("{id}.log"))).unwrap(),
            "what it said",
        );
        assert!(legacy.join(format!("{id}.json")).is_file());

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A store with nothing old under it — every run after the first — is a quiet no-op, and an
    /// agent that happens to be *called* `process` is not mistaken for a legacy tree.
    #[test]
    fn nothing_to_migrate_is_not_an_error() {
        let store = scratch("empty");
        assert_eq!(store.migrate_legacy(backends).expect("nothing"), 0);

        let session = store
            .create("process", Backend::HarnessAdi, "/tmp", "an agent named process")
            .expect("create");
        std::fs::write(store.log_path("process", &session.id), "output").unwrap();
        assert_eq!(store.migrate_legacy(backends).expect("still nothing"), 0);
        assert_eq!(
            store.get("process", &session.id).map(|s| s.message),
            Some("an agent named process".into()),
            "its sessions are files, not agent directories, so they are left alone",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }
}
