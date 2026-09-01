//! What a conversation spent its context on, and what it spent twice.
//!
//! A turn's [`TurnMetrics`](crate::progress::TurnMetrics) says how many tokens it cost. That is the bill, not the itemization: it
//! cannot say that the same forty-line file arrived six times, or that one absolute path was spelled
//! out in ninety separate tool calls. This module does the itemization — it re-reads the transcript,
//! tokenizes it, and reports the runs of tokens that were sent more than once.
//!
//! # Why a real tokenizer
//!
//! Because the answer is a *cost*, and characters are not what is billed. A repeated ASCII path and a
//! repeated stretch of CJK differ by several times in tokens per character, so ranking findings by
//! character count would order them by the wrong quantity and confidently recommend fixing the
//! cheaper one. [`tiktoken_rs`] is used, with its ranks compiled in — no model is loaded, nothing is
//! fetched, and the analysis works on a machine with no network.
//!
//! It is, however, **`OpenAI`'s** BPE, and an agent here may be talking to any provider. Every family's
//! tokenizer is a byte-level BPE trained on broadly similar text, so the counts are close and — much
//! more to the point — the *ordering* of findings is stable across them: the block that dominates the
//! waste under one encoding dominates it under all of them. So the number is reported as an estimate,
//! under the name of the encoding that produced it, and never as the provider's own accounting.
//!
//! # What counts as "sent"
//!
//! Everything that goes into the model's context on the next turn: what the user said, what the agent
//! said, what it thought, the arguments it passed to a tool, and — the one that actually dominates —
//! what the tool handed back. A queued message is excluded: it has been typed, not asked, and has
//! cost nothing yet.

mod suffix;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tiktoken_rs::CoreBPE;

use crate::progress::Step;
use crate::store::Turn;

/// The encoding whose ranks the counts are in, reported alongside them so a number is never mistaken
/// for a particular provider's own billing.
pub const ENCODING: &str = "o200k_base";

/// Real token ids are shifted up by one so that `0` is free to terminate the stream — the suffix
/// array needs a smallest, unique sentinel and must not find it among the data.
const TOKEN_BASE: u32 = 1;

/// One token of a prompt: the id the encoder produced, and the exact bytes it produced it from.
///
/// The pair is the point. A count alone cannot show that a leading space belongs to the *next*
/// word, that a heading cost four tokens, or that a path was shredded into nine — and those are the
/// things somebody reading a prompt to find what is wrong with it is looking for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptToken {
    pub id: u32,
    /// The token's text, newlines and leading spaces included.
    pub text: String,
    /// Whether this is a chat template's own control token rather than content.
    ///
    /// Always `false` from [`split`], and honestly so: what this crate composes is a *prompt*, and
    /// the wrapper around it — the role markers, the tool envelope — is added by each provider's
    /// API from the JSON body [`adi_loop`](crate::backends::harness::adi_loop) sends. There is no
    /// point in this pipeline where a rendered chat template exists to be split, and inventing the
    /// seams would be showing a reader tokens nobody is charged for.
    pub special: bool,
}

/// Split `text` into the tokens a model is charged for.
///
/// The same encoder [`analyze`] counts with, so a prompt shown here and a prompt counted there
/// cannot disagree — and it stays on this side of the wire, because the ranks are a megabyte and a
/// half and a browser has no business carrying them to render a page.
///
/// A byte run that is not valid UTF-8 on its own — a token that is half of an emoji, which is
/// ordinary — comes back through `from_utf8_lossy` rather than being dropped: the boundary is real
/// and worth drawing even where the fragment is unreadable alone.
#[must_use]
pub fn split(text: &str) -> Vec<PromptToken> {
    let bpe = tiktoken_rs::o200k_base_singleton();
    bpe.encode_ordinary(text)
        .into_iter()
        .map(|id| PromptToken {
            id,
            text: bpe
                .decode_bytes(&[id])
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default(),
            special: false,
        })
        .collect()
}

/// Shortest run worth reporting, in tokens. Below roughly this length every conversation repeats
/// itself constantly and means nothing by it: `", "`, `def `, a closing brace. The floor is what makes
/// the list findings rather than a histogram of English.
pub const DEFAULT_MIN_REPEAT: usize = 12;

/// How many repeats to describe. The tail is long and each entry costs less than the one above it.
pub const DEFAULT_MAX_REPEATS: usize = 40;

/// The most tokens analyzed. A very long conversation is truncated to its most recent segments rather
/// than refused: the recent context is what a reader can still act on, and the alternative is an
/// endpoint that gets slower until it is abandoned.
pub const MAX_ANALYZED_TOKENS: usize = 400_000;

/// A segment must be at least this long to be compared against other segments for near-duplication.
/// Short segments are near-duplicates of each other constantly — two one-line shell commands differing
/// in a flag are "95% similar" and worth nothing as a finding.
const MIN_NEAR_DUP_TOKENS: usize = 120;

/// How many of a segment's shingles have to differ before two segments are called different things.
/// In bits of a 64-bit simhash: 0 is identical, and unrelated text sits near 32.
const NEAR_DUP_DISTANCE: u32 = 6;

/// Tokens per shingle when fingerprinting a segment for near-duplication.
const SHINGLE: usize = 5;

/// Where a piece of the conversation came from — which is what turns "8k tokens repeated" into
/// something actionable, because the fix differs completely by source. Repetition in tool *output* is
/// the agent re-reading something; in tool *input* it is a literal that wanted to be a variable; in
/// the user's own text it is a prompt preamble that wanted to be a system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A question, as the user wrote it.
    User,
    /// The agent's answer, or something it said mid-turn.
    Agent,
    /// A reasoning block.
    Thinking,
    /// The arguments a tool was called with.
    ToolInput,
    /// What a tool handed back.
    ToolOutput,
}

impl Source {
    /// The word the rail puts on it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Source::User => "you",
            Source::Agent => "agent",
            Source::Thinking => "thinking",
            Source::ToolInput => "tool input",
            Source::ToolOutput => "tool output",
        }
    }
}

/// What a repeated run looks like, which is the whole basis for suggesting what to do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// A filesystem path — the textbook case for a variable, or for a working directory the agent is
    /// told once.
    Path,
    /// A URL.
    Url,
    /// A long opaque literal: a hash, a key, an id.
    Literal,
    /// Several lines of text — a file, an output, a preamble.
    Block,
    /// A single line of ordinary text.
    Phrase,
}

impl Shape {
    /// What to do about a repeat of this shape, in the imperative, or nothing when the shape does not
    /// imply a fix on its own. A hint that fires on everything is one nobody reads.
    #[must_use]
    pub fn hint(self) -> Option<&'static str> {
        match self {
            Shape::Path => Some("a path repeated this often wants to be a variable or a cwd"),
            Shape::Url => Some("hoist into a variable"),
            Shape::Literal => Some("an opaque literal — pass it once, by name"),
            Shape::Block => Some("re-sent verbatim — say it once, or read less of it"),
            Shape::Phrase => None,
        }
    }
}

/// One place a repeat was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Site {
    /// Index of the turn in the transcript.
    pub turn: usize,
    /// Index of the step within that turn, when the text came from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<usize>,
    pub source: Source,
    /// The tool's name, when the site is a tool call.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
}

/// A run of tokens that was sent more than once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repeat {
    /// The repeated text itself, trimmed for display (see [`preview`]).
    pub preview: String,
    /// Its length in tokens.
    pub tokens: usize,
    /// How many times it was sent (non-overlapping occurrences).
    pub count: usize,
    /// Tokens spent on the occurrences after the first — what could have been saved.
    pub wasted: usize,
    pub shape: Shape,
    /// Where it was sent, in transcript order, at most a screenful.
    pub sites: Vec<Site>,
}

/// A group of segments that are nearly, but not exactly, the same thing.
///
/// The case exact repeats cannot see: a file read, edited, and read again is not one repeated run — it
/// is six large blocks that differ in a line each, so the shared parts are shorter than the floor and
/// scattered. As a group it is obvious, and it is usually the largest single thing a long agent run
/// spends its context on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NearDuplicates {
    pub preview: String,
    /// How many segments are in the group.
    pub count: usize,
    /// Tokens in the group's largest member — roughly what one copy costs.
    pub tokens: usize,
    /// Tokens in every member after the first: what near-repeating cost.
    pub wasted: usize,
    pub sites: Vec<Site>,
}

/// The itemization of one conversation's context.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenReport {
    /// The encoding the counts are in.
    pub encoding: String,
    /// Every token analyzed, across every source.
    pub total: usize,
    /// The total, split by where it came from — ordered, so the rail renders it without re-deciding.
    pub by_source: Vec<(Source, usize)>,
    /// True when the conversation was longer than [`MAX_ANALYZED_TOKENS`] and only its recent end was
    /// analyzed. Everything else here then describes that end, not the whole.
    pub truncated: bool,
    /// Repeated runs, worst first.
    pub repeats: Vec<Repeat>,
    /// Tokens attributable to repetition — the sum over [`Repeat::wasted`], which the deduplication in
    /// the search keeps from double-counting nested findings.
    pub wasted: usize,
    /// Groups of near-identical segments, worst first. Counted apart from [`TokenReport::wasted`]:
    /// their overlap with the exact repeats is real, and adding the two would claim a conversation
    /// wasted more than it sent.
    pub near_duplicates: Vec<NearDuplicates>,
}

/// Knobs, so the endpoint can widen the net without a rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub min_repeat_tokens: usize,
    pub max_repeats: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            min_repeat_tokens: DEFAULT_MIN_REPEAT,
            max_repeats: DEFAULT_MAX_REPEATS,
        }
    }
}

/// One piece of text that was sent, and what it was.
struct Segment {
    site: Site,
    tokens: Vec<u32>,
    text: String,
}

/// Itemize a conversation: tokenize its transcript, then report what it sent twice.
///
/// Cost is dominated by the tokenizer, which is linear; the repeat search is `O(n log n)` on top. On a
/// long conversation this is tens of milliseconds, which is why it is a request the reader makes and
/// not something folded into the one-second poll.
#[must_use]
pub fn analyze(turns: &[Turn], opts: Options) -> TokenReport {
    let bpe = tiktoken_rs::o200k_base_singleton();
    let mut segments = segments_of(turns, bpe);
    let total: usize = segments.iter().map(|s| s.tokens.len()).sum();

    let mut truncated = false;
    let mut kept = 0usize;
    let mut first = 0usize;
    for (i, seg) in segments.iter().enumerate().rev() {
        if kept + seg.tokens.len() > MAX_ANALYZED_TOKENS {
            first = i + 1;
            truncated = true;
            break;
        }
        kept += seg.tokens.len();
    }
    if truncated {
        segments.drain(..first);
    }

    let mut by_source: HashMap<Source, usize> = HashMap::new();
    for seg in &segments {
        *by_source.entry(seg.site.source).or_default() += seg.tokens.len();
    }
    let mut by_source: Vec<(Source, usize)> = by_source.into_iter().collect();
    by_source.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let repeats = find_repeats(&segments, bpe, opts);
    let wasted = repeats.iter().map(|r| r.wasted).sum();
    let near_duplicates = find_near_duplicates(&segments);

    TokenReport {
        encoding: ENCODING.to_string(),
        total,
        by_source,
        truncated,
        repeats,
        wasted,
        near_duplicates,
    }
}

/// Everything the transcript sent to the model, in order, one segment per distinct piece of text.
fn segments_of(turns: &[Turn], bpe: &CoreBPE) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut push = |site: Site, text: &str| {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let tokens = bpe.encode_ordinary(text);
        if tokens.is_empty() {
            return;
        }
        out.push(Segment {
            site,
            tokens,
            text: text.to_string(),
        });
    };

    for (t, turn) in turns.iter().enumerate() {
        // Typed, not asked — a queued message has cost nothing yet, and counting it would report a
        // context larger than the one the model has actually been given.
        if turn.queued {
            continue;
        }
        let source = if turn.role == "user" {
            Source::User
        } else {
            Source::Agent
        };
        push(
            Site {
                turn: t,
                step: None,
                source,
                tool: String::new(),
            },
            &turn.text,
        );
        for (s, step) in turn.steps.iter().enumerate() {
            match step {
                Step::Message { text } => push(
                    Site {
                        turn: t,
                        step: Some(s),
                        source: Source::Agent,
                        tool: String::new(),
                    },
                    text,
                ),
                Step::Thinking { text } => push(
                    Site {
                        turn: t,
                        step: Some(s),
                        source: Source::Thinking,
                        tool: String::new(),
                    },
                    text,
                ),
                Step::Tool {
                    name,
                    input,
                    output,
                    ..
                } => {
                    push(
                        Site {
                            turn: t,
                            step: Some(s),
                            source: Source::ToolInput,
                            tool: name.clone(),
                        },
                        input,
                    );
                    push(
                        Site {
                            turn: t,
                            step: Some(s),
                            source: Source::ToolOutput,
                            tool: name.clone(),
                        },
                        output,
                    );
                }
            }
        }
    }
    out
}

/// Lay the segments end to end and ask the suffix machinery what occurs twice.
///
/// The segments are separated by ids that appear exactly once each, chosen just above the highest real
/// token so the counting sort's alphabet stays the size of a vocabulary rather than the size of
/// `u32`. Being unique, a separator cannot be part of any repeat — which is what confines a finding to
/// one piece of text instead of letting it straddle the seam between two unrelated ones.
fn find_repeats(segments: &[Segment], bpe: &CoreBPE, opts: Options) -> Vec<Repeat> {
    if segments.is_empty() {
        return Vec::new();
    }
    let max_real = segments
        .iter()
        .flat_map(|s| s.tokens.iter())
        .copied()
        .max()
        .unwrap_or(0)
        + TOKEN_BASE;

    let mut stream: Vec<u32> = Vec::new();
    let mut bounds: Vec<usize> = Vec::with_capacity(segments.len());
    for (i, seg) in segments.iter().enumerate() {
        bounds.push(stream.len());
        stream.extend(seg.tokens.iter().map(|t| t + TOKEN_BASE));
        if i + 1 < segments.len() {
            let Some(sep) = max_real.checked_add(1 + u32::try_from(i).unwrap_or(u32::MAX)) else {
                break;
            };
            stream.push(sep);
        }
    }

    let raw = suffix::maximal_repeats(&stream, opts.min_repeat_tokens, opts.max_repeats);
    raw.into_iter()
        .filter_map(|r| {
            let start = *r.starts.first()?;
            let ids: Vec<u32> = stream[start..start + r.len]
                .iter()
                .map(|t| t.saturating_sub(TOKEN_BASE))
                .collect();
            // A repeat can end mid-character (BPE splits multi-byte codepoints), so the bytes are
            // decoded lossily rather than dropped — a preview with one replacement character still
            // identifies the text; nothing at all does not.
            let text = bpe
                .decode_bytes(&ids)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            let sites = r
                .starts
                .iter()
                .map(|&off| {
                    let idx = bounds.partition_point(|&b| b <= off).saturating_sub(1);
                    segments[idx].site.clone()
                })
                .collect();
            Some(Repeat {
                preview: preview(&text),
                tokens: r.len,
                count: r.count,
                wasted: r.wasted(),
                shape: shape_of(&text),
                sites,
            })
        })
        .collect()
}

/// Group segments that say almost the same thing.
///
/// Fingerprint each large segment with a simhash over its token shingles, then cluster by Hamming
/// distance — the same trick the code index uses to find copy-paste, applied to what a conversation
/// sent rather than to what a file contains. Greedy single-pass clustering: the first member of a
/// group is its representative, which is enough when the threshold is this tight.
fn find_near_duplicates(segments: &[Segment]) -> Vec<NearDuplicates> {
    let big: Vec<usize> = (0..segments.len())
        .filter(|&i| segments[i].tokens.len() >= MIN_NEAR_DUP_TOKENS)
        .collect();
    if big.len() < 2 {
        return Vec::new();
    }
    let hashes: Vec<u64> = big.iter().map(|&i| simhash(&segments[i].tokens)).collect();

    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut reps: Vec<u64> = Vec::new();
    for (k, &i) in big.iter().enumerate() {
        if let Some(g) = reps
            .iter()
            .position(|&h| (h ^ hashes[k]).count_ones() <= NEAR_DUP_DISTANCE)
        {
            groups[g].push(i);
        } else {
            reps.push(hashes[k]);
            groups.push(vec![i]);
        }
    }

    let mut out: Vec<NearDuplicates> = groups
        .into_iter()
        .filter(|g| g.len() > 1)
        .map(|g| {
            let sizes: Vec<usize> = g.iter().map(|&i| segments[i].tokens.len()).collect();
            let largest = sizes.iter().copied().max().unwrap_or(0);
            let wasted = sizes.iter().sum::<usize>() - largest;
            NearDuplicates {
                preview: preview(&segments[g[0]].text),
                count: g.len(),
                tokens: largest,
                wasted,
                sites: g.iter().map(|&i| segments[i].site.clone()).collect(),
            }
        })
        .collect();
    out.sort_unstable_by(|a, b| b.wasted.cmp(&a.wasted));
    out
}

/// A 64-bit simhash over the token stream's shingles: each shingle votes on every bit, and the sign of
/// each column becomes the fingerprint. Two texts that share most of their shingles agree on most
/// bits, however far apart the shared parts sit.
///
/// The scheme is Charikar's (<https://doi.org/10.1145/509907.509965>, §3), as used for near-duplicate
/// web pages — the same construction the code index fingerprints symbol shapes with.
fn simhash(tokens: &[u32]) -> u64 {
    let mut acc = [0i32; 64];
    for window in tokens.windows(SHINGLE.min(tokens.len().max(1))) {
        // FNV-1a over the shingle's ids — cheap, and well-mixed enough for a vote per bit. The two
        // constants are FNV's own 64-bit offset basis and prime
        // (<https://datatracker.ietf.org/doc/html/draft-eastlake-fnv>, tables 1 and 2), not knobs.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &t in window {
            for byte in t.to_le_bytes() {
                h ^= u64::from(byte);
                h = h.wrapping_mul(0x1000_0000_01b3);
            }
        }
        for (b, slot) in acc.iter_mut().enumerate() {
            if h >> b & 1 == 1 {
                *slot += 1;
            } else {
                *slot -= 1;
            }
        }
    }
    acc.iter()
        .enumerate()
        .filter(|&(_, &v)| v > 0)
        .fold(0u64, |h, (b, _)| h | 1 << b)
}

/// The most characters of a repeat shown in the rail. Long enough to recognize what it is, short
/// enough that forty findings still read as a list.
const PREVIEW_CHARS: usize = 160;

/// A repeat as a single readable line: whitespace collapsed (a re-sent file is mostly indentation, and
/// indentation is not what identifies it) and cut to [`PREVIEW_CHARS`].
fn preview(text: &str) -> String {
    let mut out = String::with_capacity(PREVIEW_CHARS + 1);
    let mut space = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        if out.chars().count() >= PREVIEW_CHARS {
            out.push('\u{2026}');
            break;
        }
        out.push(ch);
    }
    out
}

/// Classify a repeat by what it looks like, which is what decides whether there is anything to
/// suggest doing about it.
///
/// The test is over the words a repeat *contains*, not over the whole of it. A repeated path almost
/// never arrives alone — it comes with the flag before it and the line number after — and a rule that
/// demanded the entire run be nothing but a path would classify the actual finding, every time, as
/// ordinary prose.
fn shape_of(text: &str) -> Shape {
    let t = text.trim();
    if t.contains('\n') {
        return Shape::Block;
    }
    if t.contains("http://") || t.contains("https://") {
        return Shape::Url;
    }
    let words = || t.split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',');
    if words().any(is_path) {
        return Shape::Path;
    }
    if words().any(is_opaque_literal) {
        return Shape::Literal;
    }
    Shape::Phrase
}

/// Whether a word is a filesystem path: rooted or relative, and deep enough that it was not just a
/// sentence with a slash in it.
fn is_path(word: &str) -> bool {
    let w = word.trim_end_matches([':', ')', ']', '.']);
    let rooted =
        w.starts_with('/') || w.starts_with("./") || w.starts_with("../") || w.starts_with("~/");
    (rooted || w.contains('/')) && w.matches('/').count() >= 2 && !w.contains("//")
}

/// Whether a word is a long opaque literal — a hash, a key, an id. Digit density is what separates one
/// from a long identifier somebody chose: `deploy_worker_pool` is meant to be read, `a3f19c8b0e` is
/// not.
fn is_opaque_literal(word: &str) -> bool {
    let w = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    if w.len() < 16
        || !w
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return false;
    }
    let digits = w.chars().filter(char::is_ascii_digit).count();
    digits * 4 >= w.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::{ToolStatus, TurnContent, TurnMetrics};
    use crate::store::{assistant_turn, user_turn};

    fn assistant_with(steps: Vec<Step>, text: &str) -> Turn {
        assistant_turn(&TurnContent {
            text: text.to_string(),
            steps,
            metrics: Some(TurnMetrics::default()),
        })
    }

    fn tool(name: &str, input: &str, output: &str) -> Step {
        Step::Tool {
            name: name.to_string(),
            input: input.to_string(),
            status: ToolStatus::Ok,
            output: output.to_string(),
        }
    }

    /// The headline case: one long path spelled out in every tool call. It should come back as a
    /// single finding, counted once per call, and classified as the thing a variable would fix.
    #[test]
    fn a_path_repeated_across_tool_calls_is_one_finding() {
        let path = "/Users/someone/projects/service/crates/api/src/handlers/agents.rs";
        let turns: Vec<Turn> = (0..8)
            .map(|i| {
                assistant_with(
                    vec![tool("Read", &format!("{path} offset={i}"), "…file…")],
                    "done",
                )
            })
            .collect();
        let report = analyze(&turns, Options::default());
        let hit = report
            .repeats
            .iter()
            .find(|r| r.preview.contains("handlers/agents.rs"))
            .expect("the repeated path should be reported");
        assert_eq!(hit.count, 8, "once per call");
        assert_eq!(hit.wasted, hit.tokens * 7);
        assert_eq!(hit.shape, Shape::Path);
        assert!(hit.sites.iter().all(|s| s.source == Source::ToolInput));
        assert!(report.wasted >= hit.wasted);
    }

    /// A conversation that never says the same thing twice must produce an empty report — otherwise
    /// the rail grows a finding on every clean run and the panel stops meaning anything.
    #[test]
    fn a_conversation_without_repetition_reports_none() {
        let turns = vec![
            user_turn("Explain how the scheduler decides which node runs a job."),
            assistant_with(
                Vec::new(),
                "It ranks candidates by free capacity, then by locality.",
            ),
        ];
        let report = analyze(&turns, Options::default());
        assert!(report.repeats.is_empty(), "got {:?}", report.repeats);
        assert_eq!(report.wasted, 0);
        assert!(report.total > 0, "tokens are still counted");
    }

    /// Queued messages have been typed, not asked. Counting them would report a context the model has
    /// not been given.
    #[test]
    fn queued_messages_are_not_counted() {
        let mut queued = user_turn("this one is still waiting in the queue");
        queued.queued = true;
        let asked = user_turn("this one was asked");
        let with_queue = analyze(&[asked.clone(), queued], Options::default());
        let without = analyze(&[asked], Options::default());
        assert_eq!(with_queue.total, without.total);
    }

    /// Totals are split by where the text came from, and tool output — the part nobody types and
    /// everybody pays for — is attributed to the tool rather than to the agent.
    #[test]
    fn totals_are_attributed_to_their_source() {
        let turns = vec![
            user_turn("read the file"),
            assistant_with(
                vec![tool(
                    "Read",
                    "src/main.rs",
                    &"a line of file content\n".repeat(40),
                )],
                "here it is",
            ),
        ];
        let report = analyze(&turns, Options::default());
        let top = report.by_source.first().expect("a source");
        assert_eq!(
            top.0,
            Source::ToolOutput,
            "the output dominates: {:?}",
            report.by_source
        );
        assert!(report.by_source.iter().any(|(s, _)| *s == Source::User));
        assert_eq!(
            report.total,
            report.by_source.iter().map(|(_, n)| n).sum::<usize>()
        );
    }

    /// The case exact repeats cannot see: the same file read three times with an edit in between.
    #[test]
    fn nearly_identical_reads_are_grouped() {
        let body = (0..200)
            .map(|i| format!("fn thing_{i}() -> usize {{ {i} }}"))
            .collect::<Vec<_>>()
            .join("\n");
        let edited = body.replace("fn thing_7()", "fn renamed_seven()");
        let turns = vec![
            assistant_with(vec![tool("Read", "lib.rs", &body)], "read"),
            assistant_with(vec![tool("Read", "lib.rs", &edited)], "read again"),
        ];
        let report = analyze(&turns, Options::default());
        assert!(
            !report.near_duplicates.is_empty(),
            "two near-identical reads should group"
        );
        assert_eq!(report.near_duplicates[0].count, 2);
    }

    /// Run the analysis over a real transcript and print what it found and how long it took.
    ///
    /// Ignored by default because it needs a file this machine may not have. It exists because the
    /// only thing that can make this feature bad is latency on a *large* conversation, and a
    /// synthetic transcript is exactly the input that never exercises it:
    ///
    /// ```text
    /// ADI_TRANSCRIPT=~/.adi/mono/sessions/<agent>/<id>.transcript.jsonl \
    ///   cargo test -p adi-agents -- --ignored --nocapture profile_a_real_transcript
    /// ```
    #[test]
    #[ignore = "needs a transcript on this machine; set ADI_TRANSCRIPT"]
    fn profile_a_real_transcript() {
        let path = std::env::var("ADI_TRANSCRIPT").expect("set ADI_TRANSCRIPT");
        let body = std::fs::read_to_string(&path).expect("read the transcript");
        let turns: Vec<Turn> = body
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let started = std::time::Instant::now();
        let report = analyze(&turns, Options::default());
        let took = started.elapsed();
        println!(
            "{} turns, {} tokens ({}), {} wasted over {} repeats, {} near-dup groups, in {took:?}",
            turns.len(),
            report.total,
            report.encoding,
            report.wasted,
            report.repeats.len(),
            report.near_duplicates.len(),
        );
        for r in report.repeats.iter().take(10) {
            println!(
                "  {:>7} wasted  {:>3}x {:>5}tok  {:?}  {}",
                r.wasted, r.count, r.tokens, r.shape, r.preview
            );
        }
        assert!(!turns.is_empty(), "the transcript parsed as no turns");
    }

    /// A preview is one line, whatever the repeat was.
    #[test]
    fn previews_are_a_single_line() {
        let p = preview("  first line\n\n\tsecond    line  ");
        assert_eq!(p, "first line second line");
    }

    #[test]
    fn shapes_are_classified() {
        assert_eq!(shape_of("/usr/local/share/thing/file.rs"), Shape::Path);
        assert_eq!(
            shape_of("--manifest-path /repo/crates/api/Cargo.toml --release"),
            Shape::Path
        );
        assert_eq!(shape_of("https://example.com/a/b"), Shape::Url);
        assert_eq!(shape_of("a\nb"), Shape::Block);
        assert_eq!(shape_of("run the tests again"), Shape::Phrase);
        assert_eq!(shape_of("token 9f2c4b8e1d7a0365"), Shape::Literal);
        assert_eq!(shape_of("call deploy_worker_pool now"), Shape::Phrase);
    }
}
