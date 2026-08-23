//! The model in the loop, behind a trait.
//!
//! Two jobs, and they are the only two a language model does in this crate:
//!
//! * **extraction** — one raw note in, a list of plain sentences out. Only reached by
//!   `facts add --text`; the default path is a caller that already emits facts, because a caller
//!   in a live conversation has context a background extractor never will (`CLI.md`, "The
//!   extraction question").
//! * **classification** — a pair the embedder found similar in, one of `duplicate` / `narrows` /
//!   `independent` / `controversy` out.
//!
//! Neither decides anything. The classifier's verdict says whether a pair is *worth a reviewer's
//! attention*; a person or a verifier agent then rules on it, and that ruling is what the base
//! records. Nothing is ever merged automatically, at any similarity — above 0.80 the measured
//! base held more controversies than duplicates, and merging the top-ranked pair would have
//! silently deleted a carve-out (`RESULTS.md` §9).
//!
//! # The prompts
//!
//! [`EXTRACT_SYSTEM`] is copied word for word from the Python prototype
//! (`experiment/knowledge-base/facts`). Its wording was iterated against a hand-labelled corpus,
//! and §10 makes the stakes sharp: change it and every measured number moves, because it changes
//! how verbosely facts are written and therefore what they score against each other. Edit it only
//! with a re-measurement in hand.
//!
//! [`CLASSIFY_SYSTEM`] is the prototype's plus one paragraph, added after a real run found the
//! classifier could not see the difference between a person's statement and an agent's conclusion
//! drawn from it. That change moves no cosine and so touches no threshold; see the constant's own
//! docs for what it does cost.
//!
//! # Failure is reported, never assumed
//!
//! The prototype caught every classifier exception and defaulted the whole chunk to
//! `independent` — which means "nothing to do", which means an unreachable model quietly emptied
//! the review queue. Here a chunk that cannot be classified comes back
//! [`Relation::Unclassified`], those pairs reach the reviewer, and the error travels with them.

use std::fmt;
use std::str::FromStr;

use serde_json::{Value, json};

use crate::ollama::{Ollama, env_or};

/// What the classifier says about one pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// They say the same thing; one merged fact would replace both.
    Duplicate,
    /// One is a more specific or qualified version of the other; still compatible.
    Narrows,
    /// Both true at once, about different things; nothing to do.
    Independent,
    /// They cannot comfortably both stand.
    Controversy,
    /// The classifier could not be reached, or answered nothing for this pair. **Not**
    /// `independent`: an unread pair is unread, and saying otherwise is how a queue empties
    /// itself without anybody noticing.
    Unclassified,
}

impl Relation {
    /// Whether a pair of this kind reaches the reviewer's queue.
    #[must_use]
    pub fn is_actionable(self) -> bool {
        !matches!(self, Self::Independent)
    }

    /// Its one-word written form, as stored in `pending.kind`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Duplicate => "duplicate",
            Self::Narrows => "narrows",
            Self::Independent => "independent",
            Self::Controversy => "controversy",
            Self::Unclassified => "unclassified",
        }
    }
}

impl fmt::Display for Relation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Relation {
    type Err = ();

    /// Anything the model says that is not one of the four is [`Relation::Unclassified`] — a
    /// verdict nobody recognises is not a verdict, and treating it as `independent` would drop
    /// the pair.
    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "duplicate" => Self::Duplicate,
            "narrows" => Self::Narrows,
            "independent" => Self::Independent,
            "controversy" => Self::Controversy,
            _ => Self::Unclassified,
        })
    }
}

/// One side of a pair, with the provenance the classifier needs to read it correctly.
///
/// The texts alone are not enough, and that was found by using the tool rather than by reading
/// it. An agent recording what a person said and then recording a conclusion it drew from that
/// statement produces two sentences that are near-identical in wording and entirely different as
/// records: one is what was said, the other is what was inferred. Judged on wording, the pair is
/// a `duplicate` and the merge deletes a person's own words. Judged with `author`, `creator`, and
/// `kind` in view, it is two records that both stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Side<'a> {
    /// The sentence.
    pub fact: &'a str,
    /// Whose meaning it is.
    pub author: &'a str,
    /// Who wrote the record.
    pub creator: &'a str,
    /// `fact`, `note`, or `artifact`.
    pub kind: &'a str,
}

impl<'a> Side<'a> {
    /// A side with no provenance to offer — a caller that has only the text.
    #[must_use]
    pub fn bare(fact: &'a str) -> Self {
        Self {
            fact,
            author: "unknown",
            creator: "unknown",
            kind: "fact",
        }
    }

    /// How this side is labelled in the prompt: `said by igor, written by agent:chat@1 [fact]`.
    fn provenance(&self) -> String {
        format!(
            "said by {}, written by {} [{}]",
            self.author, self.creator, self.kind
        )
    }
}

/// One classified pair: what it is, and why in the model's own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judgement {
    /// What the pair is.
    pub relation: Relation,
    /// Twelve words at most, and often the tell: below about rank 750 the reason routinely
    /// describes facts that are not in the pair it was given, which is a free false-positive
    /// detector (`RESULTS.md` §9).
    pub why: String,
}

/// What went wrong reaching the model.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct JudgeError(pub String);

/// The model that extracts facts from prose and classifies pairs.
///
/// Injected the same way [`Embedder`](adi_indexer::embed::Embedder) is, and for the same reason:
/// which model does this is a deployment question, and business logic that reached an HTTP
/// endpoint directly would answer it once for everybody.
pub trait Judge: fmt::Debug + Send + Sync {
    /// What to call this in a report.
    fn name(&self) -> &str;

    /// Durable facts stated by one raw note, one plain sentence each.
    ///
    /// # Errors
    /// [`JudgeError`] when the model cannot be reached. An empty list is a valid answer and not
    /// an error — a note may establish nothing.
    fn extract(&self, note: &str) -> Result<Vec<String>, JudgeError>;

    /// Classify each pair, **in the order given and one judgement per pair**.
    ///
    /// # Errors
    /// [`JudgeError`] only when nothing at all could be classified. A partial failure comes back
    /// as [`Relation::Unclassified`] for the pairs it touched, so the rest still reach a
    /// reviewer.
    fn classify(&self, pairs: &[(Side<'_>, Side<'_>)]) -> Result<Vec<Judgement>, JudgeError>;
}

/// The extraction prompt. Verbatim from the prototype — see the [module docs](self).
pub const EXTRACT_SYSTEM: &str = r#"You are the extraction step of a knowledge base. You are given ONE note: raw,
verbatim, possibly dictated speech. It may ramble and cover several topics. Filler carries no
meaning.

Write down the durable FACTS it establishes. A fact is ONE plain sentence — no fields, no
categories — the way a person would say it to someone who wasn't there. Negation belongs inside
the sentence ("We do not support the CIS"), never split out.

One fact per sentence. Each must stand alone: resolve nothing you cannot resolve from THIS note,
and drop a fact whose referent is missing rather than guessing it. Only durable things —
decisions, constraints, preferences, rejections. Not instructions, not questions, not tasks.
Use the speaker's own terms. Empty array is a valid answer.

Output STRICTLY a JSON array of strings, no prose, no fence."#;

/// The classification prompt.
///
/// **No longer verbatim**, and this is the one place in the crate that departs from the
/// prototype's wording. A real run showed the classifier calling `duplicate` on a person's
/// statement paired with an agent's conclusion drawn from it — right about the wording, wrong
/// about the record, and a merge there deletes what somebody actually said. It could not have
/// done better: the prompt never showed it who said either sentence. So each side now arrives
/// labelled, and one paragraph tells the model what the labels mean.
///
/// Be clear about what that does and does not invalidate. **Neighbour selection is untouched**:
/// it reads cosines, and no classifier prompt moves a cosine. (The *extraction* prompt does, by
/// changing how verbosely facts are written — `RESULTS.md` §10 — which is why that one is still
/// verbatim.) What this does invalidate is the classifier's own measured precision: §9's "124 of
/// 6441 pairs actionable, 66 of them controversies" was counted with the old wording, on pairs
/// shown without provenance.
pub const CLASSIFY_SYSTEM: &str = r#"You are reviewing a knowledge base of one person's facts. You get PAIRS the base
found similar. For each, say what a reviewer must do:

  duplicate    — they say the same thing AND are the same kind of record; one merged fact
                 would replace both. A [fact] and an [artifact] are NEVER duplicates.
  narrows      — one is a more specific or qualified version of the other; still compatible
  independent  — both true at once, about different things; nothing to do
  controversy  — they cannot comfortably both stand. One rules out or reverses the other, or the
                 person changed his mind.

Be strict about `controversy`. Two facts on the same topic are not a controversy. A hope and a
doubt about the same thing IS one. A decision reversed later IS one.

Each side is labelled with who said it, who wrote it down, and its kind. [fact] is something a
person stated; [artifact] is a conclusion an agent derived from other facts. A statement and a
conclusion drawn from it must both stand, however alike the wording — one is what was said, the
other is what was inferred from it. Call that pair `narrows` when the conclusion is the sharper
of the two, `independent` otherwise.

Return STRICTLY a JSON array, one object per pair, in the order given, no prose, no fence:
[{"i":<index>,"verdict":"...","why":"<=12 words"}]"#;

/// Pairs per classifier call.
///
/// The prototype's number, and it is a real constraint rather than a round one: the whole point
/// of batching is that a local model is free, and 60 pairs fit the 16k context the prompt asks
/// for with room for the answer.
const BATCH: usize = 60;

/// The output budget one call gets, in tokens.
///
/// A full [`BATCH`] of judgements is ~1,800 tokens of JSON; this is roughly double, because the
/// cost of being wrong in one direction is a truncated answer and in the other is nothing at all
/// on a local model.
const NUM_PREDICT: u32 = 4096;

/// A local model served by ollama — what the prototype used, and the default here.
///
/// Local because the classifier reads roughly 2 pairs per inserted fact and the whole measured
/// sweep was 6441 pairs in 53 minutes at zero cost (`RESULTS.md` §9). A hosted model would make
/// review a budget decision instead of a compute one.
///
/// It shares [`Ollama`] with [`OllamaEmbedder`](crate::embed::OllamaEmbedder), so
/// `ADI_FACTS_OLLAMA` moves both halves of the model work at once.
#[derive(Debug, Clone)]
pub struct OllamaJudge {
    ollama: Ollama,
    model: String,
}

/// The model the prototype's measurements were taken with.
pub const DEFAULT_MODEL: &str = "qwen3.6";

/// The environment variable that changes it.
pub const MODEL_VAR: &str = "ADI_FACTS_JUDGE";

impl Default for OllamaJudge {
    fn default() -> Self {
        Self::new()
    }
}

impl OllamaJudge {
    /// The judge described by `ADI_FACTS_OLLAMA` and `ADI_FACTS_JUDGE`, else the defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ollama: Ollama::new(),
            model: env_or(MODEL_VAR, DEFAULT_MODEL),
        }
    }

    /// Point it at a specific host and model.
    #[must_use]
    pub fn at(host: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            ollama: Ollama::at(host),
            model: model.into(),
        }
    }

    /// One `/api/generate` round: system prompt, user prompt, whole response as text.
    fn generate(&self, system: &str, prompt: &str) -> Result<String, JudgeError> {
        let body = json!({
            "model": self.model,
            "system": system,
            "prompt": prompt,
            "stream": false,
            // Reasoning tokens come out of the same budget as the answer and this asks for JSON,
            // not deliberation.
            "think": false,
            "options": {
                "temperature": 0,
                "num_ctx": 16384,
                // DO NOT DROP THIS while tidying the options map. Without `num_predict` ollama
                // applies its own default output cap, and a full batch of 60 pairs needs roughly
                // 1,800 tokens of JSON — so the array arrives truncated, deterministically, for
                // any input long enough to reach the cap. The first agent to use this tool hit it
                // four times in one run and could reproduce it byte for byte. It costs nothing on
                // a local model.
                "num_predict": NUM_PREDICT,
            },
        });
        let answer = self
            .ollama
            .post("generate", &body)
            .map_err(|e| JudgeError(e.to_string()))?;
        answer["response"]
            .as_str()
            .map(ToString::to_string)
            .ok_or_else(|| {
                JudgeError(format!(
                    "{}: no `response` field in the answer — is `{}` pulled? (`ollama pull {}`)",
                    self.ollama.host(),
                    self.model,
                    self.model
                ))
            })
    }
}

impl Judge for OllamaJudge {
    fn name(&self) -> &str {
        &self.model
    }

    fn extract(&self, note: &str) -> Result<Vec<String>, JudgeError> {
        let raw = self.generate(EXTRACT_SYSTEM, note)?;
        let Some(array) = json_array(&raw) else {
            return Err(JudgeError(format!(
                "the extractor answered with no JSON array: {}",
                preview(&raw)
            )));
        };
        Ok(array
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect())
    }

    fn classify(&self, pairs: &[(Side<'_>, Side<'_>)]) -> Result<Vec<Judgement>, JudgeError> {
        if pairs.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = vec![
            Judgement {
                relation: Relation::Unclassified,
                why: String::new(),
            };
            pairs.len()
        ];
        let mut first_error: Option<String> = None;
        let mut classified = 0usize;

        for (offset, chunk) in pairs.chunks(BATCH).enumerate() {
            let base = offset * BATCH;
            let prompt = chunk
                .iter()
                .enumerate()
                .map(|(i, (a, b))| {
                    format!(
                        "[{i}] A ({}): {}\n     B ({}): {}",
                        a.provenance(),
                        a.fact,
                        b.provenance(),
                        b.fact
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            let answer = match self.generate(CLASSIFY_SYSTEM, &prompt) {
                Ok(answer) => answer,
                Err(e) => {
                    first_error.get_or_insert(e.0);
                    continue;
                }
            };
            let array = json_objects(&answer);
            if array.is_empty() {
                first_error.get_or_insert(format!(
                    "the classifier answered with no usable JSON: {}",
                    preview(&answer)
                ));
                continue;
            }
            for entry in array {
                // The model indexes into the chunk it was shown, not the whole batch. An index
                // it invented is dropped rather than written over somebody else's pair.
                let Some(i) = entry["i"].as_u64().and_then(|i| usize::try_from(i).ok()) else {
                    continue;
                };
                if i >= chunk.len() {
                    continue;
                }
                out[base + i] = Judgement {
                    relation: entry["verdict"]
                        .as_str()
                        .unwrap_or("")
                        .parse()
                        .unwrap_or(Relation::Unclassified),
                    why: entry["why"].as_str().unwrap_or("").trim().to_string(),
                };
                classified += 1;
            }
        }

        match first_error {
            // Nothing came back at all: the caller deserves the reason, not a queue of
            // `unclassified` pairs with no explanation attached.
            Some(e) if classified == 0 => Err(JudgeError(e)),
            _ => Ok(out),
        }
    }
}

/// A judge that answers nothing, for a build or a machine with no model.
///
/// Every pair comes back [`Relation::Unclassified`], which means every selected pair
/// reaches the reviewer. That is the honest degradation: the base still refuses to decide
/// anything by itself, and the reviewer sees more than they would have rather than less.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoJudge;

impl Judge for NoJudge {
    #[allow(
        clippy::unnecessary_literal_bound,
        reason = "the trait fixes the signature; a `&'static str` here would not implement it"
    )]
    fn name(&self) -> &str {
        "none"
    }

    fn extract(&self, _note: &str) -> Result<Vec<String>, JudgeError> {
        Err(JudgeError(
            "no classifier is configured, so --text cannot extract facts from a note; \
             send one fact per line instead"
                .to_string(),
        ))
    }

    fn classify(&self, pairs: &[(Side<'_>, Side<'_>)]) -> Result<Vec<Judgement>, JudgeError> {
        Ok(vec![
            Judgement {
                relation: Relation::Unclassified,
                why: "no classifier configured".to_string(),
            };
            pairs.len()
        ])
    }
}

/// The outermost `[...]` in a model's answer, parsed whole.
///
/// Models wrap JSON in prose and fences however firmly they are told not to, so the answer is cut
/// at the first `[` and the last `]` rather than parsed whole. Used where a partial answer is
/// worse than none — extraction, where salvaging half a note's facts would silently lose the
/// rest.
fn json_array(raw: &str) -> Option<Vec<Value>> {
    let start = raw.find('[')?;
    let end = raw.rfind(']')?;
    if end < start {
        return None;
    }
    serde_json::from_str::<Value>(&raw[start..=end])
        .ok()?
        .as_array()
        .cloned()
}

/// Every **complete** object inside the answer's array, whatever state the array ends in.
///
/// The reason this is not [`json_array`]: a truncated response is a well-formed prefix and a
/// broken tail, and handing the whole span to serde loses every judgement in it — 59 perfectly
/// good verdicts thrown away because the sixtieth was cut mid-word. That is what a real run hit,
/// and it read as "the classifier could not be reached" followed by 40 pairs to decide by hand.
///
/// So a malformed tail costs the pairs in the tail and nothing else. Whatever cannot be recovered
/// stays [`Relation::Unclassified`], which already reaches the reviewer rather than being assumed
/// compatible.
///
/// Brace-matching rather than a regular expression (the prototype scanned for
/// `\{[^{}]*"verdict"[^{}]*\}`): it needs no dependency, and it cannot be fooled by a brace
/// inside a `why` string.
fn json_objects(raw: &str) -> Vec<Value> {
    let Some(start) = raw.find('[') else {
        return Vec::new();
    };
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let (mut depth, mut object_at) = (0usize, 0usize);
    let (mut in_string, mut escaped) = (false, false);

    for i in (start + 1)..bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    object_at = i;
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    // Byte indices of ASCII braces are always char boundaries.
                    if let Ok(value) = serde_json::from_str::<Value>(&raw[object_at..=i]) {
                        out.push(value);
                    }
                }
            }
            // The array closed cleanly; anything after it is prose.
            b']' if depth == 0 => break,
            _ => {}
        }
    }
    out
}

fn preview(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 200 {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(200).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_array_is_found_inside_whatever_the_model_wrapped_it_in() {
        let fenced = "Sure!\n```json\n[{\"i\":0,\"verdict\":\"duplicate\"}]\n```\nHope that helps.";
        let array = json_array(fenced).expect("array");
        assert_eq!(array.len(), 1);
        assert_eq!(array[0]["verdict"], "duplicate");

        assert!(json_array("no json here").is_none());
        assert!(json_array("] backwards [").is_none());
    }

    /// The failure a real run hit: ollama's default output cap cut the array mid-object, and
    /// parsing the whole span lost all sixty verdicts — including the fifty-nine that were
    /// intact. `num_predict` stops the truncation; this stops it from being catastrophic when
    /// something else truncates anyway.
    #[test]
    fn a_truncated_answer_costs_the_tail_and_nothing_else() {
        let truncated = r#"[
          {"i":0,"verdict":"duplicate","why":"same claim twice"},
          {"i":1,"verdict":"controversy","why":"one reverses the other"},
          {"i":2,"verdict":"narr"#;
        let salvaged = json_objects(truncated);
        assert_eq!(salvaged.len(), 2, "the two complete objects survive");
        assert_eq!(salvaged[0]["verdict"], "duplicate");
        assert_eq!(salvaged[1]["i"], 1);
        // The strict parser is what used to be used here, and it recovers nothing at all.
        assert!(json_array(truncated).is_none());
    }

    #[test]
    fn salvage_is_not_fooled_by_braces_and_brackets_inside_a_reason() {
        let awkward = r#"Sure! ```json
          [{"i":0,"verdict":"narrows","why":"A qualifies B { and ] tricky \" quote"},
           {"i":1,"verdict":"independent","why":"different things"}]
        ``` hope that helps"#;
        let salvaged = json_objects(awkward);
        assert_eq!(salvaged.len(), 2);
        assert!(salvaged[0]["why"].as_str().unwrap().contains("tricky"));
        assert_eq!(salvaged[1]["verdict"], "independent");
    }

    #[test]
    fn an_answer_with_nothing_parseable_salvages_nothing_rather_than_guessing() {
        assert!(json_objects("I could not do that.").is_empty());
        assert!(json_objects("[{\"broken").is_empty());
    }

    #[test]
    fn a_verdict_nobody_recognises_is_unclassified_and_not_independent() {
        assert_eq!("controversy".parse::<Relation>(), Ok(Relation::Controversy));
        assert_eq!("CONTRADICTION".parse::<Relation>(), Ok(Relation::Unclassified));
        // The distinction is load-bearing: `independent` drops the pair from the queue.
        assert!(!Relation::Independent.is_actionable());
        assert!(Relation::Unclassified.is_actionable());
    }

    #[test]
    fn with_no_model_every_pair_reaches_the_reviewer() {
        let pairs = [
            (Side::bare("a"), Side::bare("b")),
            (Side::bare("c"), Side::bare("d")),
        ];
        let judged = NoJudge.classify(&pairs).expect("classify");
        assert_eq!(judged.len(), 2);
        assert!(judged.iter().all(|j| j.relation == Relation::Unclassified));
        // …and `--text` fails loudly rather than staging an empty transaction.
        assert!(NoJudge.extract("some prose").is_err());
    }
}
