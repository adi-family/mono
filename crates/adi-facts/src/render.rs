//! What the caller sees. Plain text, everywhere, and no JSON anywhere.
//!
//! The reader is a language model. JSON would cost it tokens to parse and give it nothing a
//! laid-out line does not — so the prototype dropped every `--json` flag it had, and this port
//! does not put them back.
//!
//! Two rules the output obeys, both of them about not lying by omission:
//!
//! * a **capped** pair list says how many pairs were dropped and below what strength, because a
//!   silent cut reads as "nothing else to see";
//! * a **classifier that could not be reached** says so, and its pairs are shown as
//!   `unclassified` rather than quietly filtered out as `independent`.
//!
//! Every view ends with the command to run next. A caller that has to work out its own next step
//! from a data structure is a caller spending a round trip on nothing.

use std::fmt::Write as _;

use crate::model::{Committed, Neighbour, Reference, Staging, Stale};

/// A staged transaction: what is waiting, and what has to be decided first.
#[must_use]
pub fn staging(s: &Staging) -> String {
    let open = s.open();
    let mut out = format!(
        "{}  {} staged, {} to decide",
        s.tx,
        s.staged.len(),
        open.len()
    );
    if let Some(t) = s.truncated {
        let _ = write!(
            out,
            "\n   [capped: {} more pair(s) below {:.3} not examined]",
            t.dropped, t.below
        );
    }
    if let Some(e) = &s.judge_error {
        let _ = write!(
            out,
            "\n   [the classifier could not be reached: {e}]\
             \n   every close pair is listed below, unread — decide them or abort."
        );
    }

    if s.pending.is_empty() {
        // Not "nothing was close": the nearest neighbours were selected and read, and none of
        // them conflicts. Saying "nothing close" would misdescribe a base that holds a near
        // neighbour the classifier called `independent`.
        out.push_str("\n\nnothing to decide — nothing here conflicts with what the base holds.");
        let _ = write!(out, "\n\n  facts tx commit {}", s.tx);
        return out;
    }

    for p in &s.pending {
        if let Some(verdict) = &p.verdict {
            let _ = write!(
                out,
                "\n\n[p{}] {:.3}  {}  -> {verdict} by {}",
                p.pair,
                p.strength,
                p.kind,
                p.confirmer.as_deref().unwrap_or("?")
            );
            continue;
        }
        let _ = write!(out, "\n\n[p{}] {:.3}  {}", p.pair, p.strength, p.kind);
        let _ = write!(out, "\n  new   #{:<3} {}", p.new_seq, p.new_text);
        let _ = write!(out, "\n  base  {:<4} {}", p.theirs(), p.other_text);
        if !p.why.is_empty() {
            let _ = write!(out, "\n  why        {}", p.why);
        }
    }

    if open.is_empty() {
        let _ = write!(out, "\n\n  facts tx commit {}", s.tx);
    } else {
        let _ = write!(
            out,
            "\n\ndecide each, then commit:\
             \n  facts tx resolve {tx} <p> --verdict coexist|merge|supersede|drop --confirmer <who>\
             \n  facts tx commit {tx}",
            tx = s.tx
        );
    }
    out
}

/// What committing did.
#[must_use]
pub fn committed(c: &Committed) -> String {
    let mut out = format!("committed {} new", c.added.len());
    if !c.rewritten.is_empty() {
        let _ = write!(
            out,
            ", rewrote {} in place ({})",
            c.rewritten.len(),
            c.rewritten.join(", ")
        );
    }
    if c.dropped > 0 {
        let _ = write!(out, ", dropped {}", c.dropped);
    }
    if c.linked > 0 {
        let _ = write!(out, ", linked to {} source(s)", c.linked);
    }
    for (id, fact) in &c.added {
        let _ = write!(out, "\n  {id}  {fact}");
    }
    out
}

/// What is out of date, and because of what.
#[must_use]
pub fn stale(rows: &[Stale]) -> String {
    if rows.is_empty() {
        return "everything is up to date".to_string();
    }
    rows.iter()
        .map(|r| {
            format!(
                "{}  {}\n    out of date because {} changed",
                r.id, r.fact, r.root_cause
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The queue around one fact.
#[must_use]
pub fn near(rows: &[Neighbour]) -> String {
    if rows.is_empty() {
        return "nothing else in this base".to_string();
    }
    rows.iter()
        .map(|n| format!("{:.3}  {}  {}", n.strength, n.id, n.fact))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every fact, for somebody reading the base rather than searching it.
#[must_use]
pub fn list(rows: &[crate::model::Fact]) -> String {
    if rows.is_empty() {
        return "nothing in this base yet".to_string();
    }
    rows.iter()
        .map(|f| format!("{:<18} v{:<3} {:<9} {}", f.id, f.version, f.kind, f.fact))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Search results — the same shape as [`near`], because they answer with the same three things.
///
/// "nothing close" would be a lie: neither this nor [`near`] cuts anything for scoring low, so an
/// empty result means the base holds nothing else, not that nothing matched.
#[must_use]
pub fn search(rows: &[Neighbour]) -> String {
    if rows.is_empty() {
        return "nothing in this base yet".to_string();
    }
    near(rows)
}

/// What a reference resolves to, and what changed under it.
#[must_use]
pub fn reference(r: &Reference, full: bool) -> String {
    let f = &r.fact;
    let mut out = format!("{}  v{}  [{}]\n  {}", f.id, f.version, f.kind, f.fact);
    let _ = write!(out, "\n  said by {}, written by {}", f.author, f.creator);

    let heading = match (r.referenced, r.drifted) {
        (Some(v), true) => {
            let _ = write!(
                out,
                "\n\n  STALE REFERENCE — written against v{v}, the fact is now v{}.",
                f.version
            );
            "\nwhat changed since:"
        }
        (Some(v), false) => {
            let _ = write!(out, "\n\n  reference is current (v{v})");
            if !full {
                return out;
            }
            "\nhistory:"
        }
        (None, _) => {
            // A fact nobody has touched since it was written needs no log printed at it.
            if r.since.len() <= 1 && !full {
                out.push_str("\n\n  unchanged since it was written");
                return out;
            }
            "\nhistory:"
        }
    };

    out.push('\n');
    out.push_str(heading);
    for e in &r.since {
        let _ = write!(
            out,
            "\n  v{:<3} {:<11} by {}",
            e.version, e.event, e.confirmer
        );
        if !e.was.is_empty() {
            let _ = write!(out, "\n        was: {}", e.was);
        }
        if !e.now.is_empty() && e.now != e.was {
            let _ = write!(out, "\n        now: {}", e.now);
        }
    }

    // Version, not "has more than one log entry": a fact that absorbed another inside its own
    // batch has two entries at v1 and still says exactly what it was created saying. Warning
    // about it would send a reader looking for a change that never happened.
    if r.referenced.is_none() && f.version > 1 {
        out.push_str(
            "\n\n  NOTE: this id still resolves, but what it says has changed since v1. A reference\
             \n        written against an earlier version may no longer mean what its author meant.",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Event, Fact, Pending, Truncation};

    fn pending(pair: i64, verdict: Option<&str>) -> Pending {
        Pending {
            pair,
            new_seq: 4,
            base_id: Some("f_091".into()),
            base_seq: None,
            strength: 0.886,
            kind: "controversy".into(),
            why: "one excludes the CIS, the other carves Ukraine out of it".into(),
            verdict: verdict.map(ToString::to_string),
            keep: None,
            confirmer: verdict.map(|_| "igor".to_string()),
            new_text: "The company supports all countries except the CIS.".into(),
            other_text: "Within the CIS, the company supports Ukraine.".into(),
        }
    }

    fn staging_of(pending: Vec<Pending>) -> Staging {
        Staging {
            tx: "tx_7f3a91".into(),
            state: "needs_review".into(),
            staged: vec!["a".into(), "b".into()],
            pending,
            truncated: None,
            judge_error: None,
        }
    }

    #[test]
    fn a_pair_awaiting_a_decision_shows_both_sentences_and_how_to_rule_on_it() {
        let text = staging(&staging_of(vec![pending(0, None)]));
        assert!(text.contains("tx_7f3a91  2 staged, 1 to decide"), "{text}");
        assert!(text.contains("0.886  controversy"), "{text}");
        assert!(text.contains("new   #4"), "{text}");
        assert!(text.contains("base  f_091"), "{text}");
        assert!(text.contains("facts tx resolve tx_7f3a91"), "{text}");
    }

    #[test]
    fn a_decided_pair_shows_its_verdict_and_who_made_it() {
        let text = staging(&staging_of(vec![pending(0, Some("coexist"))]));
        assert!(text.contains("-> coexist by igor"), "{text}");
        assert!(
            !text.contains("facts tx resolve"),
            "nothing left to decide: {text}"
        );
        assert!(text.contains("facts tx commit tx_7f3a91"), "{text}");
    }

    /// The one lie this interface must never tell.
    #[test]
    fn a_capped_list_says_how_much_it_cut_and_below_what() {
        let mut s = staging_of(vec![pending(0, None)]);
        s.truncated = Some(Truncation {
            dropped: 12,
            below: 0.612,
        });
        let text = staging(&s);
        assert!(
            text.contains("capped: 12 more pair(s) below 0.612"),
            "{text}"
        );
    }

    /// An unreachable classifier used to empty the queue: every chunk defaulted to
    /// `independent`, which means "nothing to do".
    #[test]
    fn an_unreachable_classifier_is_reported_rather_than_read_as_nothing_to_decide() {
        let mut s = staging_of(vec![pending(0, None)]);
        s.judge_error = Some("connection refused".into());
        let text = staging(&s);
        assert!(text.contains("classifier could not be reached"), "{text}");
        assert!(text.contains("connection refused"), "{text}");
    }

    #[test]
    fn nothing_close_is_said_plainly_and_offers_the_commit() {
        let text = staging(&staging_of(vec![]));
        assert!(text.contains("nothing to decide"), "{text}");
        assert!(
            !text.contains("close"),
            "the pairs were read, not skipped: {text}"
        );
        assert!(text.contains("facts tx commit"), "{text}");
    }

    fn reference_of(version: i64, referenced: Option<i64>, since: Vec<Event>) -> Reference {
        let drifted = referenced.is_some_and(|v| v != version);
        Reference {
            fact: Fact {
                id: "f_1a02".into(),
                fact: "The company was reincorporated in Nevada.".into(),
                author: "igor".into(),
                creator: "agent:chat@1".into(),
                version,
                updated_at: 0,
                kind: "fact".into(),
            },
            referenced,
            since,
            drifted,
        }
    }

    #[test]
    fn a_reference_written_against_an_older_version_is_told_it_has_drifted() {
        let text = reference(
            &reference_of(
                2,
                Some(1),
                vec![Event {
                    version: 2,
                    event: "supersede".into(),
                    was: "The company was incorporated in Delaware.".into(),
                    now: "The company was reincorporated in Nevada.".into(),
                    confirmer: "igor".into(),
                    at: 0,
                }],
            ),
            false,
        );
        assert!(
            text.contains("STALE REFERENCE — written against v1, the fact is now v2."),
            "{text}"
        );
        assert!(
            text.contains("was: The company was incorporated in Delaware."),
            "{text}"
        );
        assert!(
            text.contains("now: The company was reincorporated in Nevada."),
            "{text}"
        );
    }

    /// A fact that swallowed another inside its own batch has two log entries and one version:
    /// its meaning never moved, so nothing should tell a reader it did.
    #[test]
    fn absorbing_another_fact_is_not_a_change_of_meaning() {
        let text = reference(
            &reference_of(
                1,
                None,
                vec![
                    Event {
                        version: 1,
                        event: "created".into(),
                        was: String::new(),
                        now: "The company was reincorporated in Nevada.".into(),
                        confirmer: "agent:chat@1".into(),
                        at: 0,
                    },
                    Event {
                        version: 1,
                        event: "absorbed".into(),
                        was: "The company moved to Nevada.".into(),
                        now: "The company was reincorporated in Nevada.".into(),
                        confirmer: "igor".into(),
                        at: 0,
                    },
                ],
            ),
            true,
        );
        assert!(
            text.contains("absorbed"),
            "the loser is still on the record: {text}"
        );
        assert!(
            !text.contains("no longer mean what its author meant"),
            "{text}"
        );
    }

    #[test]
    fn a_current_reference_says_so_and_prints_no_log() {
        let text = reference(&reference_of(1, Some(1), vec![]), false);
        assert!(text.contains("reference is current (v1)"), "{text}");
        assert!(!text.contains("what changed"), "{text}");
    }

    /// Without a version the tool cannot know whether the reader's copy is current, so it says
    /// plainly that the id still resolves and the meaning has moved.
    #[test]
    fn a_reference_with_no_version_is_warned_that_the_meaning_moved() {
        let text = reference(
            &reference_of(
                2,
                None,
                vec![
                    Event {
                        version: 1,
                        event: "created".into(),
                        was: String::new(),
                        now: "The company was incorporated in Delaware.".into(),
                        confirmer: "agent:chat@1".into(),
                        at: 0,
                    },
                    Event {
                        version: 2,
                        event: "supersede".into(),
                        was: "The company was incorporated in Delaware.".into(),
                        now: "The company was reincorporated in Nevada.".into(),
                        confirmer: "igor".into(),
                        at: 0,
                    },
                ],
            ),
            false,
        );
        assert!(
            text.contains("this id still resolves, but what it says has changed"),
            "{text}"
        );
    }
}
