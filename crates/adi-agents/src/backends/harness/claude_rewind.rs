//! Where a `harness:claude-sdk` conversation is cut when a reader rewinds it.
//!
//! The CLI keeps each session as a JSONL file of entries chained by `parentUuid`, and its hidden
//! `--resume-session-at <uuid>` resumes with only the chain up to that entry — later turns stay in
//! the file as a dead branch. So rewinding is finding the right entry, and nothing is ever deleted
//! from the CLI's own storage.
//!
//! The entry is found by **time**, not by counting prompts. This crate's turns and the CLI's
//! entries share no id, and a count drifts the moment anything but a typed message starts a turn
//! (an await wake, a goal check, a turn another engine answered during a failover). What does hold
//! is ordering: a user turn is recorded before its child is spawned, and one turn runs at a time,
//! so every entry of the turns being kept is older than the first dropped turn's own timestamp.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::{Error, Result};

/// The fields of a session entry that place it in the chain. Everything else on the line — whole
/// API requests among it — is skipped by serde without being built.
#[derive(Deserialize)]
struct Entry {
    uuid: Option<String>,
    #[serde(rename = "parentUuid")]
    parent: Option<String>,
    /// What a compaction boundary carries instead of a parent: the chain it summarized.
    #[serde(rename = "logicalParentUuid")]
    logical_parent: Option<String>,
    timestamp: Option<String>,
    #[serde(default, rename = "isSidechain")]
    sidechain: bool,
}

/// The chain entry to resume `session_id` at so that nothing said from `before_ms` (unix millis)
/// onwards survives, or `None` when nothing on the chain is that old — the whole session postdates
/// the cut, and the conversation has to open a new one.
///
/// # Errors
/// Returns [`Error::Unsupported`], ready to show, when the session's file cannot be found or read:
/// resuming without the cut would silently keep everything the reader asked to drop.
pub(crate) fn anchor(session_id: &str, before_ms: u64) -> Result<Option<String>> {
    let path = session_file(session_id).ok_or_else(|| {
        Error::Unsupported(format!(
            "can't rewind: Claude's session file for {session_id} wasn't found under its projects \
             directory"
        ))
    })?;
    let file = std::fs::File::open(&path).map_err(|e| {
        Error::Unsupported(format!("can't rewind: couldn't read {}: {e}", path.display()))
    })?;
    let lines = BufReader::new(file).lines().map_while(std::result::Result::ok);
    Ok(anchor_in(lines, before_ms))
}

/// `<config>/projects/<cwd slug>/<session id>.jsonl`. Globbed across projects rather than derived
/// from the working directory: the slug is the CLI's own encoding of a path, and the session id is
/// already unique.
fn session_file(session_id: &str) -> Option<PathBuf> {
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".claude")))?;
    let name = format!("{session_id}.jsonl");
    std::fs::read_dir(root.join("projects"))
        .ok()?
        .flatten()
        .map(|project| project.path().join(&name))
        .find(|candidate| candidate.is_file())
}

fn anchor_in(lines: impl Iterator<Item = String>, before_ms: u64) -> Option<String> {
    let mut entries: HashMap<String, (Option<String>, Option<u64>)> = HashMap::new();
    let mut leaf = None;
    for line in lines {
        let Ok(entry) = serde_json::from_str::<Entry>(&line) else {
            continue;
        };
        let Some(uuid) = entry.uuid else { continue };
        if entry.sidechain {
            continue;
        }
        let at = entry.timestamp.as_deref().and_then(millis);
        entries.insert(uuid.clone(), (entry.parent.or(entry.logical_parent), at));
        leaf = Some(uuid);
    }
    // From the newest entry back along its parents: only the live chain, so a branch an earlier
    // rewind already abandoned is never chosen however old its entries are.
    let mut seen = HashSet::new();
    let mut cursor = leaf;
    while let Some(uuid) = cursor {
        if !seen.insert(uuid.clone()) {
            break;
        }
        let (parent, at) = entries.get(&uuid)?;
        if at.is_some_and(|at| at < before_ms) {
            return Some(uuid);
        }
        cursor = parent.clone();
    }
    None
}

fn millis(timestamp: &str) -> Option<u64> {
    let at = OffsetDateTime::parse(timestamp, &Rfc3339).ok()?;
    u64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(uuid: &str, parent: Option<&str>, second: u32) -> String {
        serde_json::json!({
            "type": "user",
            "uuid": uuid,
            "parentUuid": parent,
            "timestamp": format!("2026-01-01T00:00:{second:02}.000Z"),
        })
        .to_string()
    }

    fn at(second: u64) -> u64 {
        millis("2026-01-01T00:00:00.000Z").expect("epoch") + second * 1000
    }

    #[test]
    fn the_anchor_is_the_newest_chain_entry_older_than_the_cut() {
        let lines = vec![
            r#"{"type":"queue-operation","timestamp":"2026-01-01T00:00:00.000Z"}"#.to_string(),
            entry("q1", None, 1),
            entry("a1", Some("q1"), 2),
            entry("q2", Some("a1"), 10),
            entry("a2", Some("q2"), 11),
        ];
        assert_eq!(anchor_in(lines.clone().into_iter(), at(9)).as_deref(), Some("a1"));
        assert_eq!(anchor_in(lines.clone().into_iter(), at(60)).as_deref(), Some("a2"));
        assert_eq!(anchor_in(lines.into_iter(), at(1)), None);
    }

    /// After one rewind the dropped turn is still in the file, and older than a later cut — but it
    /// is off the chain, and resuming at it would bring back what was dropped.
    #[test]
    fn an_abandoned_branch_is_never_the_anchor() {
        let lines = vec![
            entry("q1", None, 1),
            entry("a1", Some("q1"), 2),
            entry("dropped-q", Some("a1"), 10),
            entry("dropped-a", Some("dropped-q"), 11),
            entry("q2", Some("a1"), 20),
            entry("a2", Some("q2"), 21),
            entry("q3", Some("a2"), 30),
        ];
        assert_eq!(anchor_in(lines.clone().into_iter(), at(29)).as_deref(), Some("a2"));
        assert_eq!(anchor_in(lines.into_iter(), at(15)).as_deref(), Some("a1"));
    }

    #[test]
    fn a_sidechain_entry_is_not_the_leaf() {
        let mut side: serde_json::Value =
            serde_json::from_str(&entry("side", Some("q1"), 5)).expect("json");
        side["isSidechain"] = true.into();
        let lines = vec![entry("q1", None, 1), entry("a1", Some("q1"), 2), side.to_string()];
        assert_eq!(anchor_in(lines.into_iter(), at(60)).as_deref(), Some("a1"));
    }
}
