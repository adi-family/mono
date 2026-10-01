//! Local transcript token estimates and repeated-content analysis.
//!
//! Uses the bundled `o200k_base` tokenizer; counts are not provider billing or a
//! reconstruction of the full model context. Includes messages, thinking, and tool
//! input/output, excluding queued messages. Exact and near-duplicate estimates overlap.

mod suffix;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tiktoken_rs::CoreBPE;

use crate::progress::Step;
use crate::store::Turn;

/// Encoding used for token estimates.
pub const ENCODING: &str = "o200k_base";

/// Reserve zero for the suffix array terminator.
const TOKEN_BASE: u32 = 1;

/// A token ID and its display text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptToken {
    pub id: u32,
    /// The token's text, newlines and leading spaces included.
    pub text: String,
    /// Always false: provider chat-template control tokens are not part of this text.
    pub special: bool,
}

/// Split text with the same estimated encoding as [`analyze`].
/// Individual tokens may contain partial UTF-8 characters, decoded lossily for display.
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

/// Minimum repeated length; suppresses common punctuation and short phrases.
pub const DEFAULT_MIN_REPEAT: usize = 12;

/// Maximum number of exact repeats reported.
pub const DEFAULT_MAX_REPEATS: usize = 40;

/// Maximum tokens analyzed, retaining the most recent content.
pub const MAX_ANALYZED_TOKENS: usize = 400_000;

/// Minimum segment size for near-duplicate comparisons.
const MIN_NEAR_DUP_TOKENS: usize = 120;

/// Maximum Hamming distance between 64-bit simhash fingerprints.
const NEAR_DUP_DISTANCE: u32 = 6;

/// Tokens per shingle when fingerprinting a segment for near-duplication.
const SHINGLE: usize = 5;

/// Origin of a transcript segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A question, as the user wrote it.
    User,
    /// The agent's answer, or something it said mid-turn.
    Agent,
    Thinking,
    ToolInput,
    ToolOutput,
}

impl Source {
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

/// Content category used to suggest ways to reduce repetition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// A filesystem path.
    Path,
    Url,
    /// A long opaque literal: a hash, a key, an id.
    Literal,
    /// Several lines of text — a file, an output, a preamble.
    Block,
    /// A single line of ordinary text.
    Phrase,
}

impl Shape {
    /// Suggested action, when the content category implies one.
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
    pub tokens: usize,
    /// How many times it was sent (non-overlapping occurrences).
    pub count: usize,
    /// Tokens spent on the occurrences after the first — what could have been saved.
    pub wasted: usize,
    pub shape: Shape,
    /// Where it was sent, in transcript order, at most a screenful.
    pub sites: Vec<Site>,
}

/// Segments grouped by similar token fingerprints; may include exact copies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NearDuplicates {
    pub preview: String,
    /// How many segments are in the group.
    pub count: usize,
    /// Tokens in the group's largest member — roughly what one copy costs.
    pub tokens: usize,
    /// Estimated repetition: combined tokens minus the largest member.
    pub wasted: usize,
    pub sites: Vec<Site>,
}

/// The itemization of one conversation's context.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenReport {
    pub encoding: String,
    /// Every token analyzed, across every source.
    pub total: usize,
    /// Tokens by source, largest first.
    pub by_source: Vec<(Source, usize)>,
    /// Whether older content was omitted. All report fields describe the retained suffix.
    pub truncated: bool,
    /// Repeated runs, worst first.
    pub repeats: Vec<Repeat>,
    /// Sum of exact-repeat savings, without overlapping occurrences.
    pub wasted: usize,
    /// Similar segments, largest estimated repetition first.
    /// Overlaps exact repeats; do not add to [`TokenReport::wasted`].
    pub near_duplicates: Vec<NearDuplicates>,
}

/// Exact-repeat reporting limits.
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

/// Tokenized transcript segment.
struct Segment {
    site: Site,
    tokens: Vec<u32>,
    text: String,
}

/// Count transcript tokens by source and find repeated content within the recent token budget.
#[must_use]
pub fn analyze(turns: &[Turn], opts: Options) -> TokenReport {
    analyze_with_limit(turns, opts, MAX_ANALYZED_TOKENS)
}

fn analyze_with_limit(turns: &[Turn], opts: Options, max_tokens: usize) -> TokenReport {
    let bpe = tiktoken_rs::o200k_base_singleton();
    let (segments, truncated) = segments_of(turns, bpe, max_tokens);
    let total: usize = segments.iter().map(|s| s.tokens.len()).sum();

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

/// Retain the most recent tokens without tokenizing older segments once the budget is filled.
fn segments_of(turns: &[Turn], bpe: &CoreBPE, max_tokens: usize) -> (Vec<Segment>, bool) {
    let mut out = Vec::new();
    let mut remaining = max_tokens;
    let mut truncated = false;
    for (site, text) in segment_texts(turns).rev() {
        if remaining == 0 {
            truncated = true;
            break;
        }
        let mut tokens = bpe.encode_ordinary(text);
        let text = if tokens.len() > remaining {
            tokens.drain(..tokens.len() - remaining);
            tokens.shrink_to_fit();
            truncated = true;
            // The boundary can split a UTF-8 character; only the preview uses this decoded text.
            String::from_utf8_lossy(&bpe.decode_bytes(&tokens).unwrap_or_default()).into_owned()
        } else {
            text.to_string()
        };
        remaining -= tokens.len();
        out.push(Segment { site, tokens, text });
        if truncated {
            break;
        }
    }
    out.reverse();
    (out, truncated)
}

/// Transcript text in chronological order; queued messages are excluded.
fn segment_texts(turns: &[Turn]) -> impl DoubleEndedIterator<Item = (Site, &str)> {
    turns
        .iter()
        .enumerate()
        .filter(|(_, turn)| !turn.queued)
        .flat_map(|(t, turn)| {
            let site = move |step, source, tool: &str| Site {
                turn: t,
                step,
                source,
                tool: tool.to_string(),
            };
            let steps = turn
                .steps
                .iter()
                .enumerate()
                .flat_map(move |(s, step)| match step {
                    Step::Message { text } => [
                        Some((site(Some(s), Source::Agent, ""), text.as_str())),
                        None,
                    ],
                    Step::Thinking { text } => [
                        Some((site(Some(s), Source::Thinking, ""), text.as_str())),
                        None,
                    ],
                    Step::Tool {
                        name,
                        input,
                        output,
                        ..
                    } => [
                        Some((site(Some(s), Source::ToolInput, name), input.as_str())),
                        Some((site(Some(s), Source::ToolOutput, name), output.as_str())),
                    ],
                })
                .flatten();
            let source = if turn.role == "user" {
                Source::User
            } else {
                Source::Agent
            };
            // Assistant text is the final answer, after its thinking and tool calls.
            steps.chain(std::iter::once((
                site(None, source, ""),
                turn.text.as_str(),
            )))
        })
        .filter(|(_, text)| !text.is_empty())
}

/// Unique separators prevent repeats from crossing segment boundaries.
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
            // A repeat boundary can split a UTF-8 character.
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

/// Greedy simhash clustering, using each group's first segment as its representative.
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

/// Simhash over token shingles: each shingle votes on each fingerprint bit.
/// Based on Charikar, §3: <https://doi.org/10.1145/509907.509965>.
fn simhash(tokens: &[u32]) -> u64 {
    let mut acc = [0i32; 64];
    for window in tokens.windows(SHINGLE.min(tokens.len().max(1))) {
        // FNV-1a over little-endian token IDs.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &t in window {
            for byte in t.to_le_bytes() {
                h ^= u64::from(byte);
                h = h.wrapping_mul(0x0100_0000_01b3);
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

/// Maximum preview characters, excluding the truncation marker.
const PREVIEW_CHARS: usize = 160;

/// Collapse whitespace and limit previews to [`PREVIEW_CHARS`].
fn preview(text: &str) -> String {
    let mut out = String::with_capacity(PREVIEW_CHARS + 1);
    let mut space = false;
    let mut chars = 0;
    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space && chars < PREVIEW_CHARS {
            out.push(' ');
            chars += 1;
            space = false;
        }
        if chars == PREVIEW_CHARS {
            out.push('\u{2026}');
            break;
        }
        out.push(ch);
        chars += 1;
    }
    out
}

/// Detect paths and literals within larger repeated phrases, such as shell commands.
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

/// Require two slashes to avoid classifying ordinary prose as a path.
fn is_path(word: &str) -> bool {
    let w = word.trim_end_matches([':', ')', ']', '.']);
    let rooted =
        w.starts_with('/') || w.starts_with("./") || w.starts_with("../") || w.starts_with("~/");
    (rooted || w.contains('/')) && w.matches('/').count() >= 2 && !w.contains("//")
}

/// Distinguish opaque IDs from readable identifiers by length and digit density.
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

    #[test]
    fn truncated_totals_match_the_retained_sources() {
        let recent = user_turn("recent question");
        let limit = split(&recent.text).len();
        let report = analyze_with_limit(
            &[user_turn("an older question"), recent],
            Options::default(),
            limit,
        );
        assert!(report.truncated);
        assert_eq!(report.total, limit);
        assert_eq!(report.by_source, vec![(Source::User, limit)]);
    }

    #[test]
    fn an_oversized_segment_keeps_its_recent_tokens() {
        let turns = [user_turn("alpha beta gamma delta epsilon zeta")];
        let report = analyze_with_limit(&turns, Options::default(), 3);
        assert!(report.truncated);
        assert_eq!(report.total, 3);
        assert_eq!(report.by_source, vec![(Source::User, 3)]);
        let bpe = tiktoken_rs::o200k_base_singleton();
        let (segments, _) = segments_of(&turns, bpe, 3);
        let full = bpe.encode_ordinary(&turns[0].text);
        assert_eq!(segments[0].tokens, full[full.len() - 3..]);
        assert_eq!(segments[0].site.turn, 0);
    }

    #[test]
    fn an_exact_budget_is_not_truncated() {
        let turns = [user_turn("exact budget")];
        let limit = split(&turns[0].text).len();
        assert!(!analyze_with_limit(&turns, Options::default(), limit).truncated);
        assert!(!analyze_with_limit(&[], Options::default(), 0).truncated);
        let empty = analyze_with_limit(&turns, Options::default(), 0);
        assert!(empty.truncated);
        assert_eq!(empty.total, 0);
        assert!(empty.by_source.is_empty());
    }

    #[test]
    fn truncation_keeps_the_final_answer_after_its_tools() {
        let answer = "the final answer";
        let limit = split(answer).len();
        let report = analyze_with_limit(
            &[assistant_with(
                vec![tool("Read", "file.rs", "some output")],
                answer,
            )],
            Options::default(),
            limit,
        );
        assert!(report.truncated);
        assert_eq!(report.by_source, vec![(Source::Agent, limit)]);
    }

    #[test]
    fn whitespace_is_counted_like_the_prompt_view() {
        for text in ["  indented text\n", " \n\t  "] {
            let report = analyze(&[user_turn(text)], Options::default());
            assert_eq!(report.total, split(text).len(), "{text:?}");
        }
    }

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

    #[test]
    fn queued_messages_are_not_counted() {
        let mut queued = user_turn("this one is still waiting in the queue");
        queued.queued = true;
        let asked = user_turn("this one was asked");
        let with_queue = analyze(&[asked.clone(), queued], Options::default());
        let without = analyze(&[asked], Options::default());
        assert_eq!(with_queue.total, without.total);
    }

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

    /// Set `ADI_TRANSCRIPT` to a JSONL transcript and run with `--ignored --nocapture`.
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

    #[test]
    fn previews_are_a_single_line() {
        let p = preview("  first line\n\n\tsecond    line  ");
        assert_eq!(p, "first line second line");
    }

    #[test]
    fn previews_respect_the_character_limit_at_whitespace() {
        let full = "é".repeat(PREVIEW_CHARS);
        assert_eq!(preview(&full), full);
        assert_eq!(preview(&format!("{full} next")), format!("{full}…"));
    }

    #[test]
    fn simhash_uses_the_standard_fnv1a_prime() {
        // One token is one shingle: the fingerprint is FNV-1a of four zero bytes.
        assert_eq!(simhash(&[0]), 0x4d25_767f_9dce_13f5);
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
