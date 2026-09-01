//! What a fact base holds, as Rust types.
//!
//! A node is three things and no more: whose meaning it is, who wrote the record, and one plain
//! sentence. Earlier drafts split the sentence into subject / predicate / value and pulled
//! negation out into a polarity field; both were measured against the flat sentence and bought
//! nothing on either extraction accuracy or pair ranking, so both are gone (`DESIGN.md`, "What
//! is deliberately NOT in the node"). Negation belongs inside the sentence, where the person
//! put it.

use std::fmt;
use std::str::FromStr;

use crate::error::Error;

/// One fact, as it stands right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    /// Its id. Stable for the life of the base — `merge` and `supersede` rewrite in place.
    pub id: String,
    /// The sentence.
    pub fact: String,
    /// Whose meaning this is — usually the person who said it.
    pub author: String,
    /// Who physically wrote the record — usually an agent.
    pub creator: String,
    /// Bumped on every edit. This, and nothing else, is what edges compare.
    pub version: i64,
    /// Wall clock, milliseconds. For a human to read; nothing compares it.
    pub updated_at: i64,
    /// `fact`, `note`, or `artifact`.
    pub kind: String,
}

/// A node that is out of date, and the change that made it so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stale {
    /// The node that is stale.
    pub id: String,
    /// Its sentence, so a report needs no second lookup.
    pub fact: String,
    /// The id of the source whose version moved.
    pub root_cause: String,
    /// 0 when the changed source is a direct input, deeper when it reached here through others.
    pub depth: i64,
}

/// A fact near another one, with how near.
#[derive(Debug, Clone, PartialEq)]
pub struct Neighbour {
    /// The neighbour's id.
    pub id: String,
    /// Its sentence.
    pub fact: String,
    /// Cosine similarity, `0.0..=1.0`.
    pub strength: f32,
}

/// What a reviewer may rule on a pair.
///
/// `merge` and `supersede` are **one mechanism**: both rewrite the losing node in place and bump
/// its version, so anything derived from it goes stale, and neither ever leaves two rows. They
/// differ only in where the winning sentence comes from — `merge` takes it from `--fact`,
/// `supersede` from whichever side won.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Both stand. **This is a decision, not a skip** — it records that somebody knows the two
    /// are both true, which is the difference between "we have two notes" and "we know".
    Coexist,
    /// One sentence replaces both. Supplied with the verdict.
    Merge,
    /// One side wins outright and is written over the loser.
    Supersede,
    /// The incoming fact never lands. The base was already right.
    Drop,
}

impl Verdict {
    /// Its one-word written form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Coexist => "coexist",
            Self::Merge => "merge",
            Self::Supersede => "supersede",
            Self::Drop => "drop",
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Verdict {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        match s.trim().to_ascii_lowercase().as_str() {
            "coexist" => Ok(Self::Coexist),
            "merge" => Ok(Self::Merge),
            "supersede" => Ok(Self::Supersede),
            "drop" => Ok(Self::Drop),
            other => Err(Error::Backend(format!(
                "unknown verdict {other:?}: expected coexist, merge, supersede, or drop"
            ))),
        }
    }
}

/// One pair somebody has to rule on.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// Its number within the transaction — what `tx resolve` is given.
    pub pair: i64,
    /// The staged fact's sequence number.
    pub new_seq: i64,
    /// The other side's id when it is already in the base.
    pub base_id: Option<String>,
    /// The other side's sequence number when it is another fact in this same batch.
    pub base_seq: Option<i64>,
    /// Cosine similarity between the two.
    pub strength: f32,
    /// What the classifier called it: `controversy`, `duplicate`, `narrows`, `unclassified`.
    pub kind: String,
    /// The classifier's reason, in its own words. May be empty.
    pub why: String,
    /// The verdict, once somebody has given one.
    pub verdict: Option<String>,
    /// Which side `supersede` kept.
    pub keep: Option<String>,
    /// Who decided. A verdict with no owner is not a verdict.
    pub confirmer: Option<String>,
    /// The incoming sentence.
    pub new_text: String,
    /// The other side's sentence.
    pub other_text: String,
}

impl Pending {
    /// How the incoming fact is named on the command line: `#<seq>`.
    #[must_use]
    pub fn mine(&self) -> String {
        format!("#{}", self.new_seq)
    }

    /// How the other side is named: its base id, or `#<seq>` when it is also staged.
    #[must_use]
    pub fn theirs(&self) -> String {
        self.base_id
            .clone()
            .unwrap_or_else(|| format!("#{}", self.base_seq.unwrap_or(-1)))
    }
}

/// A staged transaction: what is waiting to land, and what has to be decided first.
#[derive(Debug, Clone, PartialEq)]
pub struct Staging {
    /// The transaction id.
    pub tx: String,
    /// `needs_review`, `ready`, `committed`, or `aborted`.
    pub state: String,
    /// The staged facts that are still going to land, in order.
    pub staged: Vec<String>,
    /// Every pair, strongest first — decided and undecided alike.
    pub pending: Vec<Pending>,
    /// How many candidate pairs were dropped by the cap, and the strength they were dropped
    /// below. `None` when nothing was dropped.
    ///
    /// Always reported. A silent cap reads as "nothing else to see", which is the one lie this
    /// interface must never tell.
    pub truncated: Option<Truncation>,
    /// What went wrong reaching the classifier, when something did. The pairs it could not read
    /// are in `pending` marked `unclassified` rather than quietly assumed compatible.
    pub judge_error: Option<String>,
}

impl Staging {
    /// The pairs nobody has ruled on yet.
    #[must_use]
    pub fn open(&self) -> Vec<&Pending> {
        self.pending
            .iter()
            .filter(|p| p.verdict.is_none())
            .collect()
    }
}

/// A capped candidate list: how much was thrown away, and below what strength.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Truncation {
    /// How many pairs were not examined.
    pub dropped: usize,
    /// The weakest strength that *was* examined — everything dropped is below this.
    pub below: f32,
}

/// What committing a transaction did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Committed {
    /// The facts that landed, in order: id and sentence.
    pub added: Vec<(String, String)>,
    /// How many provenance edges were written — one per source per landed fact.
    pub linked: usize,
    /// Base facts rewritten in place by a `merge` or `supersede`.
    pub rewritten: Vec<String>,
    /// How many staged facts were dropped by a verdict and never landed.
    pub dropped: usize,
}

/// One entry in a fact's decision log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The version the fact reached.
    pub version: i64,
    /// `created`, `merge`, `supersede`, `absorbed`, or `derived`.
    pub event: String,
    /// The text before.
    pub was: String,
    /// The text after.
    pub now: String,
    /// Who confirmed the verdict that caused it.
    pub confirmer: String,
    /// Wall clock, milliseconds.
    pub at: i64,
}

/// What a reference to a fact id resolves to today.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The fact as it now stands.
    pub fact: Fact,
    /// The version the reference was written against, if it named one.
    pub referenced: Option<i64>,
    /// Its decision log — the whole of it, or only what happened after `referenced`.
    pub since: Vec<Event>,
    /// Whether the fact has moved since the referenced version.
    pub drifted: bool,
}

/// Split `f_abc@3` into an id and the version the reference was written against.
///
/// The `@version` is what makes an outside reference self-checking: the tool reports that the
/// fact moved, instead of a reader having to notice.
#[must_use]
pub fn split_reference(text: &str) -> (&str, Option<i64>) {
    match text.split_once('@') {
        Some((id, version)) => (id, version.parse().ok()),
        None => (text, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_carries_the_version_it_was_written_against() {
        assert_eq!(split_reference("f_abc@3"), ("f_abc", Some(3)));
        assert_eq!(split_reference("f_abc"), ("f_abc", None));
        // A version that is not a number is not a version — the id still resolves.
        assert_eq!(split_reference("f_abc@v3"), ("f_abc", None));
    }

    #[test]
    fn a_verdict_round_trips_through_its_written_form() {
        for verdict in [
            Verdict::Coexist,
            Verdict::Merge,
            Verdict::Supersede,
            Verdict::Drop,
        ] {
            assert_eq!(verdict.as_str().parse::<Verdict>().unwrap(), verdict);
        }
        assert!("review".parse::<Verdict>().is_err());
    }

    #[test]
    fn a_pair_names_both_of_its_sides_the_way_keep_expects_them() {
        let mut pair = Pending {
            pair: 0,
            new_seq: 4,
            base_id: Some("f_091".into()),
            base_seq: None,
            strength: 0.8,
            kind: "controversy".into(),
            why: String::new(),
            verdict: None,
            keep: None,
            confirmer: None,
            new_text: String::new(),
            other_text: String::new(),
        };
        assert_eq!((pair.mine(), pair.theirs()), ("#4".into(), "f_091".into()));

        // Both sides staged: the other side is named by its sequence number, not by an id it
        // does not have yet.
        pair.base_id = None;
        pair.base_seq = Some(7);
        assert_eq!((pair.mine(), pair.theirs()), ("#4".into(), "#7".into()));
    }
}
