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
//! # The prompts are verbatim
//!
//! [`EXTRACT_SYSTEM`] and [`CLASSIFY_SYSTEM`] are copied word for word from the Python prototype
//! (`experiment/knowledge-base/facts`). Their wording was iterated against a hand-labelled
//! corpus and every measurement in `RESULTS.md` was taken with them — §10 makes the point
//! sharply: change the extraction prompt and the similarity floor needs re-measuring just as
//! surely as if the embedder had changed. Edit them only with a re-measurement in hand.
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
    fn classify(&self, pairs: &[(&str, &str)]) -> Result<Vec<Judgement>, JudgeError>;
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

/// The classification prompt. Verbatim from the prototype — see the [module docs](self).
pub const CLASSIFY_SYSTEM: &str = r#"You are reviewing a knowledge base of one person's facts. You get PAIRS the base
found similar. For each, say what a reviewer must do:

  duplicate    — they say the same thing; one merged fact would replace both
  narrows      — one is a more specific or qualified version of the other; still compatible
  independent  — both true at once, about different things; nothing to do
  controversy  — they cannot comfortably both stand. One rules out or reverses the other, or the
                 person changed his mind.

Be strict about `controversy`. Two facts on the same topic are not a controversy. A hope and a
doubt about the same thing IS one. A decision reversed later IS one.

Return STRICTLY a JSON array, one object per pair, in the order given, no prose, no fence:
[{"i":<index>,"verdict":"...","why":"<=12 words"}]"#;

/// Pairs per classifier call.
///
/// The prototype's number, and it is a real constraint rather than a round one: the whole point
/// of batching is that a local model is free, and 60 pairs fit the 16k context the prompt asks
/// for with room for the answer.
const BATCH: usize = 60;

/// A local model served by ollama — what the prototype used, and the default here.
///
/// Local because the classifier reads roughly 2 pairs per inserted fact and the whole measured
/// sweep was 6441 pairs in 53 minutes at zero cost (`RESULTS.md` §9). A hosted model would make
/// the floor a budget decision instead of a compute one.
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
            "options": {"temperature": 0, "num_ctx": 16384},
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

    fn classify(&self, pairs: &[(&str, &str)]) -> Result<Vec<Judgement>, JudgeError> {
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
                .map(|(i, (a, b))| format!("[{i}] A: {a}\n     B: {b}"))
                .collect::<Vec<_>>()
                .join("\n\n");
            let answer = match self.generate(CLASSIFY_SYSTEM, &prompt) {
                Ok(answer) => answer,
                Err(e) => {
                    first_error.get_or_insert(e.0);
                    continue;
                }
            };
            let Some(array) = json_array(&answer) else {
                first_error.get_or_insert(format!(
                    "the classifier answered with no JSON array: {}",
                    preview(&answer)
                ));
                continue;
            };
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
/// Every pair comes back [`Relation::Unclassified`], which means every pair above the floor
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

    fn classify(&self, pairs: &[(&str, &str)]) -> Result<Vec<Judgement>, JudgeError> {
        Ok(vec![
            Judgement {
                relation: Relation::Unclassified,
                why: "no classifier configured".to_string(),
            };
            pairs.len()
        ])
    }
}

/// The outermost `[...]` in a model's answer, parsed.
///
/// Models wrap JSON in prose and fences however firmly they are told not to, so the answer is cut
/// at the first `[` and the last `]` rather than parsed whole.
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
        let pairs = [("a", "b"), ("c", "d")];
        let judged = NoJudge.classify(&pairs).expect("classify");
        assert_eq!(judged.len(), 2);
        assert!(judged.iter().all(|j| j.relation == Relation::Unclassified));
        // …and `--text` fails loudly rather than staging an empty transaction.
        assert!(NoJudge.extract("some prose").is_err());
    }
}
