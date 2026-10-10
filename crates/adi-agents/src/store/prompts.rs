//! What a person has already said to an agent: the start prompts and replies a composer offers to
//! send again.
//!
//! Read out of the two places a message is kept — the `user` rows of `turns`, and `sessions.message`
//! for a one-shot run that records no turns — rather than a history table of its own. A separate
//! list would have to be written on every send path (the panel, the CLI, a fleet peer), and the one
//! a path forgot would hold the prompt somebody went looking for.
//!
//! # Only what a person said
//!
//! A `user` turn is anything put to the model, and most of them nobody typed: an await's wake, a
//! settled question, a goal nudge, an agent briefing the agent it launched. Offering those to re-send
//! would fill the list with the platform's own words. So a turn counts when it carries a `from`
//! marker (the panel stamps every message a person sends with one) or, unstamped, when it sits in a
//! session a person started — and never when it carries any other marker.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::launcher;
use crate::marker::{self, Marker};

use super::transcript::{ROLE_USER, Turn};

/// How many of the newest rows each source is read over before de-duplicating. A prompt that gets
/// re-sent gets re-sent recently, so a window this deep finds a screenful of distinct ones without
/// decoding every turn an agent was ever sent.
const WINDOW: i64 = 400;

/// One prompt, and how often it was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentPrompt {
    /// The words, trimmed, with any marker taken off the front.
    pub text: String,
    /// Unix milliseconds it was last sent.
    pub at: u64,
    /// How many times it was sent inside the window read.
    pub times: usize,
}

/// The newest `limit` distinct prompts a person sent `agent`, newest first.
pub(super) fn recent(conn: &Connection, agent: &str, limit: usize) -> Vec<RecentPrompt> {
    let mut said = replied(conn, agent);
    said.extend(started(conn, agent));
    said.sort_by(|a, b| b.0.cmp(&a.0));
    distinct(said, limit)
}

/// Fold `(at, text)` rows, newest first, into distinct prompts — the same words sent twice are one
/// row that says so, dated by the newer send.
fn distinct(said: Vec<(u64, String)>, limit: usize) -> Vec<RecentPrompt> {
    let mut out: Vec<RecentPrompt> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (at, text) in said {
        if let Some(&i) = index.get(&text) {
            out[i].times += 1;
        } else if out.len() < limit {
            index.insert(text.clone(), out.len());
            out.push(RecentPrompt { text, at, times: 1 });
        }
    }
    out
}

/// The user turns a person sent, newest first.
fn replied(conn: &Connection, agent: &str) -> Vec<(u64, String)> {
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT t.json, s.launched_by FROM turns t
           JOIN sessions s ON s.agent = t.agent AND s.id = t.session
          WHERE t.agent = ?1 AND t.role = ?2
          ORDER BY t.at DESC LIMIT ?3",
    ) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![agent, ROLE_USER, WINDOW], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })
    .map(|rows| {
        rows.flatten()
            .filter_map(|(json, launched_by)| {
                let turn = serde_json::from_str::<Turn>(&json).ok()?;
                Some((turn.at, spoken(&turn, &launched_by)?))
            })
            .collect()
    })
    .unwrap_or_default()
}

/// What person-started sessions with no recorded turns were opened with — a one-shot run's prompt
/// lives nowhere else. A session that has turns is left out, because its first turn already *is*
/// that prompt, and reading both would count every prompt sent once as sent twice.
fn started(conn: &Connection, agent: &str) -> Vec<(u64, String)> {
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT s.started_at, s.message FROM sessions s
          WHERE s.agent = ?1 AND s.launched_by = ?2
            AND NOT EXISTS (SELECT 1 FROM turns t WHERE t.agent = s.agent AND t.session = s.id)
          ORDER BY s.started_at DESC LIMIT ?3",
    ) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![agent, launcher::HUMAN, WINDOW], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })
    .map(|rows| {
        rows.flatten()
            .filter_map(|(at, message)| {
                let message = message.trim();
                (!message.is_empty()).then(|| (u64::try_from(at).unwrap_or(0), message.to_string()))
            })
            .collect()
    })
    .unwrap_or_default()
}

/// The words of a turn a person sent, or `None` for one the platform or another agent put there.
///
/// Turns recorded before markers were data carry them in their text, so an unmarked turn is split
/// first — otherwise a week-old wake would read as somebody's prompt.
fn spoken(turn: &Turn, launched_by: &str) -> Option<String> {
    let (markers, body) = if turn.markers.is_empty() {
        marker::split(&turn.text)
    } else {
        (turn.markers.clone(), turn.text.as_str())
    };
    if markers.iter().any(|m| !matches!(m, Marker::From { .. })) {
        return None;
    }
    // Empty is every session opened before `launched_by` existed, which a person started far more
    // often than not; the marker check above has already dropped the wakes among them.
    let person = !markers.is_empty() || launched_by == launcher::HUMAN || launched_by.is_empty();
    let body = body.trim();
    (person && !body.is_empty()).then(|| body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_fold_into_the_newest_send() {
        let said = vec![
            (30, "ship it".to_string()),
            (20, "review the diff".to_string()),
            (10, "ship it".to_string()),
        ];
        let out = distinct(said, 10);
        assert_eq!(out.len(), 2);
        assert_eq!(
            (out[0].text.as_str(), out[0].at, out[0].times),
            ("ship it", 30, 2)
        );
        assert_eq!(out[1].times, 1);
    }

    #[test]
    fn the_limit_still_counts_repeats_past_it() {
        let said = vec![
            (3, "a".to_string()),
            (2, "b".to_string()),
            (1, "a".to_string()),
        ];
        let out = distinct(said, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].times, 2);
    }

    #[test]
    fn a_wake_is_not_a_prompt() {
        let mut turn = super::super::user_turn("woke up");
        turn.markers = vec![Marker::GoalCheck { open: 1 }];
        assert_eq!(spoken(&turn, launcher::HUMAN), None);
    }

    #[test]
    fn an_agents_briefing_is_not_a_prompt_but_a_stamped_reply_in_it_is() {
        let mut turn = super::super::user_turn("do the thing");
        assert_eq!(spoken(&turn, "agent:adi-agent"), None);
        turn.markers = vec![Marker::From {
            node: "mac".into(),
            user: String::new(),
        }];
        assert_eq!(
            spoken(&turn, "agent:adi-agent").as_deref(),
            Some("do the thing")
        );
    }
}
