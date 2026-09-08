//! Placeholders: naming the long literals a prompt repeats, so they cost three tokens instead of
//! seventeen — and putting them back before the client ever sees one.
//!
//! # What it does
//!
//! A prompt from a coding agent repeats absolute paths and URLs dozens of times, and every
//! repetition is billed twice: once going up as prompt, once coming back down as answer. This
//! module finds those literals, gives each a short name, declares the names where the model will
//! read them, and reverses the substitution on the way back. The client sends and receives ordinary
//! text; only the leg between this gateway and the provider is written in shorthand.
//!
//! # Two modes, because prompt caching decides the arithmetic
//!
//! Rewriting a prompt changes it, and a changed prompt is a **cache miss**. On this machine 98.6%
//! of all input tokens are cache reads, billed at a tenth — so a rewrite that saves 1% of the
//! prompt and invalidates the cached prefix costs about ten times what it saves. That is why there
//! are two modes and why neither is on by default:
//!
//! - [`Mode::Full`] rewrites the messages and declares the dictionary in front of them. Biggest
//!   saving on the prompt, and it breaks the cache. For a provider with no prompt caching, or a
//!   one-shot call, this is the one that pays.
//! - [`Mode::Tail`] leaves every existing byte alone and only *appends* the dictionary and the
//!   answer rule at the end. The cached prefix still matches, the prompt costs a few tokens more,
//!   and the saving comes entirely from the answer — which is never cached and is billed at
//!   several times the input rate.
//!
//! Measured on a path-dense task (`projects/adi/bin/llm-macro-bench.ts`): prompt −18% to −25%,
//! answer −62% to −73% when the model writes the placeholders. Whether it does is the other open
//! question — GLM complies once told to in as many words; Claude, inside Claude Code's own system
//! prompt, wrote the paths out in full every time.
//!
//! # What it will not touch
//!
//! Only absolute paths and URLs are ever named: text a model has no reason to take apart character
//! by character. Fields the provider matches against a list of its own — `model`, tool names, ids —
//! are left alone in every mode, because a placeholder there is a 404 rather than a saving. A body
//! this does not understand is forwarded unchanged, which is always the safe answer.

use std::collections::HashMap;

use serde_json::Value;

/// The character every placeholder starts with. One token on the tokenizers measured, and not
/// something a path or a piece of code contains by accident.
const MARK: char = '§';

/// `§` as a JSON string escape. Some encoders emit non-ASCII this way, so an answer can come back
/// carrying either form and both have to expand.
const MARK_ESCAPED: &str = "\\u00a7";

/// Shortest literal worth naming. Below this the placeholder and the dictionary line cost more than
/// the repetition they remove.
const MIN_LEN: usize = 24;

/// How many times a literal has to appear before it earns its dictionary line. Two occurrences of a
/// 17-token path save 28 tokens and cost 22 to declare; three is the first comfortable margin.
const MIN_HITS: usize = 3;

/// How many literals may be named in one request. The dictionary is read by the model on every
/// call, so a long one is a cost of its own.
const MAX_ENTRIES: usize = 24;

/// Characters per token for path-like text, times ten — measured, not assumed: a 56-character
/// absolute path came to 17 tokens.
const CHARS_PER_TOKEN_X10: usize = 33;

/// What one placeholder costs, in tokens.
const PLACEHOLDER_TOKENS: isize = 3;

/// Characters per token for ordinary prose, times ten. Prose packs looser than a path does.
const PROSE_PER_TOKEN_X10: usize = 40;

/// What the dictionary is, said once above the list.
const OPENING: &str = "Placeholders: each name below stands for the exact text after its `=`, and \
                       is expanded again before anyone reads your answer.\n\n";

/// What to do with them, which is the only part that differs between the modes.
fn rule(mode: Mode) -> &'static str {
    match mode {
        Mode::Full => {
            "\nThe text above is already written this way. Write the placeholder and never its \
             value — in prose, JSON, code and tool arguments alike."
        }
        Mode::Tail => {
            "\nThe text above stays as it is. In your own answer, write the placeholder and never \
             its value — in prose, JSON, code and tool arguments alike."
        }
    }
}

/// What the instruction paragraph costs, in tokens, before a single literal is named.
///
/// It is paid on every request that carries a dictionary, whether or not the model then uses one —
/// and on a small prompt it is larger than everything the substitution saves. Measured through the
/// gateway: a 394-token prompt naming fourteen paths came back 393.
fn prose_tokens(mode: Mode) -> isize {
    signed((OPENING.len() + rule(mode).len()).div_ceil(PROSE_PER_TOKEN_X10 / 10))
}

/// How the substitution is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Rewrite the prompt and declare the dictionary in front of it. Saves the most, and changes
    /// the prefix — so it forfeits prompt caching.
    Full,
    /// Change nothing that is already there; append the dictionary and the answer rule at the end.
    /// Keeps the cached prefix intact and aims only at the answer.
    Tail,
}

impl Mode {
    /// The mode a request asked for, from the value of its `x-adi-macros` header or the configured
    /// default. Anything unrecognised — including the empty string and `off` — means no.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" | "on" | "1" | "true" => Some(Self::Full),
            "tail" => Some(Self::Tail),
            _ => None,
        }
    }
}

/// The request shape a provider speaks, which is what decides where the dictionary can safely go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `system` beside the messages, and no system role inside them.
    Anthropic,
    /// Chat completions: every instruction is a message, system included.
    OpenAi,
}

impl Shape {
    /// The shape a provider prefix implies, or `None` for one whose body this does not understand —
    /// Gemini nests its prompt under `contents`/`systemInstruction` and is left alone.
    #[must_use]
    pub fn for_provider(provider: &str) -> Option<Self> {
        match provider {
            "anthropic" => Some(Self::Anthropic),
            "openai" | "zai" => Some(Self::OpenAi),
            _ => None,
        }
    }
}

/// A placeholder and what it stands for, in the order they were named.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dict {
    entries: Vec<(String, String)>,
}

impl Dict {
    /// The literals worth naming in this body, most valuable first.
    ///
    /// Counted over the *decoded* strings, not the raw JSON: in a serialized body a newline is the
    /// two characters `\n`, so a path at the start of a line is preceded by an `n` and a scan of the
    /// raw text would refuse it as a mid-word match. Walking the tree also means only the fields
    /// that would actually be rewritten are counted.
    ///
    /// A whole path counts, and so does every directory above it — a listing of twenty files under
    /// one long directory repeats no path at all, and yet that directory is the most expensive text
    /// in the request.
    #[must_use]
    pub fn build(body: &[u8]) -> Self {
        let corpus = corpus(body);

        // The maximal runs first, then what they have in common: each run lends its own count to
        // every directory that contains it, which is how a prefix nobody wrote twice is found.
        let mut runs: HashMap<&str, usize> = HashMap::new();
        for literal in candidates(&corpus) {
            *runs.entry(literal).or_default() += 1;
        }
        let mut hits: HashMap<&str, usize> = HashMap::new();
        for (literal, n) in &runs {
            *hits.entry(literal).or_default() += n;
            for prefix in directories(literal) {
                *hits.entry(prefix).or_default() += n;
            }
        }

        let mut worth: Vec<(&str, isize)> = hits
            .into_iter()
            .filter(|(_, n)| *n >= MIN_HITS)
            .map(|(literal, n)| (literal, net_saving(literal, n)))
            .filter(|(_, net)| *net > 0)
            .collect();
        // Most valuable first, ties broken by the literal itself, so the same body always produces
        // the same dictionary — one that reordered between calls would be a cache miss by itself.
        worth.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));

        // A path and a directory inside it describe the same characters. Naming both spends a
        // dictionary line on occurrences the first name has already taken, so the better one wins
        // and the overlapping one is dropped.
        let mut chosen: Vec<&str> = Vec::new();
        for (literal, _) in worth {
            if chosen.len() == MAX_ENTRIES {
                break;
            }
            if chosen
                .iter()
                .any(|kept| kept.contains(literal) || literal.contains(kept))
            {
                continue;
            }
            chosen.push(literal);
        }

        Self {
            entries: chosen
                .into_iter()
                .enumerate()
                .map(|(i, literal)| (format!("{MARK}{}", i + 1), literal.to_string()))
                .collect(),
        }
    }

    /// Whether there is anything to substitute.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many literals are named.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Roughly what this dictionary saves on the prompt, in tokens — for the log line, not for
    /// billing.
    #[must_use]
    pub fn estimated_saving(&self, body: &[u8]) -> isize {
        let Ok(text) = std::str::from_utf8(body) else {
            return 0;
        };
        self.entries
            .iter()
            .map(|(_, literal)| net_saving(literal, text.matches(literal.as_str()).count()))
            .sum()
    }

    /// The block the model reads: what each name stands for, and what to do with them.
    #[must_use]
    pub fn preamble(&self, mode: Mode) -> String {
        let mut out = String::from(OPENING);
        for (name, literal) in &self.entries {
            out.push_str("  ");
            out.push_str(name);
            out.push_str(" = ");
            out.push_str(literal);
            out.push('\n');
        }
        out.push_str(rule(mode));
        out
    }

    /// Replace every literal with its placeholder.
    #[must_use]
    pub fn shorten(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (name, literal) in &self.entries {
            if out.contains(literal.as_str()) {
                out = out.replace(literal.as_str(), name);
            }
        }
        out
    }

    /// Put every literal back. This is what the client sees, so it has to be exact.
    #[must_use]
    pub fn expand(&self, text: &str) -> String {
        if self.entries.is_empty() || (!text.contains(MARK) && !text.contains(MARK_ESCAPED)) {
            return text.to_string();
        }
        // Both spellings of every name, longest first — without the ordering, `§1` would eat the
        // `§1` inside `§12`.
        let mut forms: Vec<(String, &str)> = Vec::with_capacity(self.entries.len() * 2);
        for (name, literal) in &self.entries {
            let digits = name.strip_prefix(MARK).unwrap_or(name);
            forms.push((format!("{MARK_ESCAPED}{digits}"), literal.as_str()));
            forms.push((name.clone(), literal.as_str()));
        }
        forms.sort_by_key(|(form, _)| std::cmp::Reverse(form.len()));

        let mut out = text.to_string();
        for (form, literal) in forms {
            if out.contains(form.as_str()) {
                out = out.replace(form.as_str(), literal);
            }
        }
        out
    }

    /// Apply the dictionary to a request body, returning the body to send upstream.
    ///
    /// `None` when there is nothing to do, or the body is not the JSON this shape describes — in
    /// which case the caller forwards the client's bytes untouched.
    ///
    /// [`Mode::Full`] also declines when the literals save less than the instruction paragraph
    /// costs, which on a short prompt they usually do. [`Mode::Tail`] does not: it is bought
    /// deliberately, and its return is on the answer rather than on the prompt.
    #[must_use]
    pub fn apply(&self, body: &[u8], mode: Mode, shape: Shape) -> Option<Vec<u8>> {
        if self.is_empty() {
            return None;
        }
        if mode == Mode::Full && self.estimated_saving(body) <= prose_tokens(mode) {
            return None;
        }
        let mut json: Value = serde_json::from_slice(body).ok()?;
        if !json.is_object() {
            return None;
        }
        if mode == Mode::Full {
            shorten_in_place(&mut json, self);
        }
        inject(&mut json, &self.preamble(mode), mode, shape)?;
        serde_json::to_vec(&json).ok()
    }
}

/// What one literal is worth: every occurrence saves the difference between its tokens and a
/// placeholder's, and the dictionary line that declares it is paid once.
fn net_saving(literal: &str, occurrences: usize) -> isize {
    let tokens = tokens_of(literal);
    let line = tokens + 6;
    (tokens - PLACEHOLDER_TOKENS) * signed(occurrences) - line
}

/// About how many tokens a path-like literal costs.
fn tokens_of(literal: &str) -> isize {
    signed((literal.len() * 10).div_ceil(CHARS_PER_TOKEN_X10))
}

/// A count as a signed number. Nothing here comes near the ceiling; saturating is simply the only
/// honest answer if it ever did.
fn signed(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

/// Every string the rewrite would be allowed to touch, run together into one text to count over.
///
/// A body that is not JSON at all is scanned as it stands: worth less, and never worse.
fn corpus(body: &[u8]) -> String {
    let mut out = String::new();
    match serde_json::from_slice::<Value>(body) {
        Ok(json) => collect(&json, &mut out),
        Err(_) => {
            if let Ok(text) = std::str::from_utf8(body) {
                out.push_str(text);
            }
        }
    }
    out
}

/// Append every rewritable string in the tree, one per line so no path runs into the next.
fn collect(value: &Value, out: &mut String) {
    match value {
        Value::String(s) => {
            out.push_str(s);
            out.push('\n');
        }
        Value::Array(items) => {
            for item in items {
                collect(item, out);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                if !NEVER_REWRITTEN.contains(&key.as_str()) {
                    collect(item, out);
                }
            }
        }
        _ => {}
    }
}

/// Every directory a path lies under, longest first, as slices of it: `/a/bb/ccc/d.rs` lends its
/// count to `/a/bb/ccc/` and `/a/bb/`. Only those long enough to be worth a name are offered.
fn directories(literal: &str) -> Vec<&str> {
    literal
        .char_indices()
        .filter(|(i, c)| *c == '/' && *i + 1 < literal.len())
        .map(|(i, _)| &literal[..=i])
        .filter(|prefix| prefix.len() >= MIN_LEN && prefix.matches('/').count() >= 3)
        .rev()
        .collect()
}

/// Every absolute path and URL in the text, as slices of it.
fn candidates(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let is_url = text[i..].starts_with("http://") || text[i..].starts_with("https://");
        if !is_url && bytes[i] != b'/' {
            i += 1;
            continue;
        }
        // A path is a candidate only at a boundary: the tail of `…/src/pages/llm.rs` must not be
        // named separately from the whole of it.
        if !is_url && start > 0 && is_path_byte(bytes[start - 1]) {
            i += 1;
            continue;
        }
        let mut end = start + if is_url { 8 } else { 1 };
        let mut slashes = usize::from(!is_url);
        while end < bytes.len() && is_path_byte(bytes[end]) {
            if bytes[end] == b'/' {
                slashes += 1;
            }
            end += 1;
        }
        // Trailing punctuation belongs to the sentence, not to the path.
        while end > start && matches!(bytes[end - 1], b'.' | b',' | b':' | b';' | b')') {
            end -= 1;
        }
        let literal = &text[start..end];
        if literal.len() >= MIN_LEN && (is_url || slashes >= 3) {
            out.push(literal);
        }
        i = end.max(start + 1);
    }
    out
}

/// Whether a byte may appear inside a path or URL being collected.
fn is_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'-' | b'_' | b'~' | b'+' | b'%' | b'@')
}

/// Fields whose value the provider matches against a list of its own. A placeholder in one of these
/// is a 404, not a saving.
const NEVER_REWRITTEN: [&str; 6] = ["model", "id", "tool_use_id", "type", "name", "tool_call_id"];

/// Rewrite every string in the tree, leaving the keys and the opaque fields alone.
fn shorten_in_place(value: &mut Value, dict: &Dict) {
    match value {
        Value::String(s) => {
            let short = dict.shorten(s);
            if short != *s {
                *s = short;
            }
        }
        Value::Array(items) => {
            for item in items {
                shorten_in_place(item, dict);
            }
        }
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if NEVER_REWRITTEN.contains(&key.as_str()) {
                    continue;
                }
                shorten_in_place(item, dict);
            }
        }
        _ => {}
    }
}

/// Put the preamble where the model will read it.
///
/// In [`Mode::Tail`] it goes last and nothing before it moves, so a cached prefix still matches. On
/// the Anthropic shape that means a text block appended to the final user message — and if the last
/// message is the assistant's, the client is prefilling an answer and injection is skipped rather
/// than allowed to break it.
fn inject(json: &mut Value, preamble: &str, mode: Mode, shape: Shape) -> Option<()> {
    let object = json.as_object_mut()?;

    if shape == Shape::Anthropic && mode == Mode::Full {
        // The system field takes a string or a list of blocks; both have to keep working.
        match object.get_mut("system") {
            Some(Value::String(s)) => *s = format!("{preamble}\n\n{s}"),
            Some(Value::Array(blocks)) => {
                blocks.insert(0, serde_json::json!({"type": "text", "text": preamble}));
            }
            _ => {
                object.insert("system".to_string(), Value::String(preamble.to_string()));
            }
        }
        return Some(());
    }

    let messages = object.get_mut("messages")?.as_array_mut()?;
    match (shape, mode) {
        (Shape::OpenAi, Mode::Full) => messages.insert(
            0,
            serde_json::json!({"role": "system", "content": preamble}),
        ),
        (Shape::OpenAi, Mode::Tail) => {
            messages.push(serde_json::json!({"role": "system", "content": preamble}));
        }
        (Shape::Anthropic, _) => {
            let last = messages.last_mut()?;
            if last.get("role").and_then(Value::as_str) != Some("user") {
                // A trailing assistant message is a prefill the client expects continued.
                return None;
            }
            match last.get_mut("content") {
                Some(Value::String(s)) => *s = format!("{s}\n\n{preamble}"),
                Some(Value::Array(blocks)) => {
                    blocks.push(serde_json::json!({"type": "text", "text": preamble}));
                }
                _ => return None,
            }
        }
    }
    Some(())
}

/// Puts the literals back in a response, whatever shape the response arrives in.
///
/// The two shapes need genuinely different machinery, which is worth saying plainly because the
/// difference is not obvious until it bites: in a whole JSON answer a placeholder is contiguous
/// text and a byte-level replacement finds it, but in a **token stream it is not**. A model emits
/// `§` and `1` as two separate deltas, each wrapped in its own `data:` frame, so the two characters
/// are a hundred bytes of JSON scaffolding apart on the wire and no amount of byte-level searching
/// will ever join them. A stream has to be taken apart frame by frame instead.
#[derive(Debug)]
pub enum Rewriter {
    /// A whole body: expand the bytes as they go past.
    Whole(Expander),
    /// A server-sent event stream: expand the text inside each frame.
    Frames(Frames),
}

impl Rewriter {
    /// The right machinery for the response the provider is actually sending.
    #[must_use]
    pub fn new(dict: Dict, event_stream: bool) -> Self {
        if event_stream {
            Self::Frames(Frames::new(dict))
        } else {
            Self::Whole(Expander::new(dict))
        }
    }

    /// Feed one chunk of the provider's answer, get back what to write to the client.
    #[must_use]
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        match self {
            Self::Whole(e) => e.push(chunk),
            Self::Frames(f) => f.push(chunk),
        }
    }

    /// Whatever is still held back when the provider stops talking.
    #[must_use]
    pub fn finish(&mut self) -> Vec<u8> {
        match self {
            Self::Whole(e) => e.finish(),
            Self::Frames(f) => f.finish(),
        }
    }
}

/// Expands placeholders in a response body, one chunk at a time.
///
/// Two things can straddle a chunk boundary and both are held back until the next one arrives: an
/// incomplete UTF-8 sequence (`§` is two bytes), and a placeholder whose digits have not been sent
/// yet. Whatever is still held at the end of the stream is flushed as it stands.
#[derive(Debug)]
pub struct Expander {
    dict: Dict,
    carry: Vec<u8>,
}

impl Expander {
    /// Wrap a dictionary for streaming use.
    #[must_use]
    pub fn new(dict: Dict) -> Self {
        Self {
            dict,
            carry: Vec::new(),
        }
    }

    /// Feed one chunk of the provider's answer, get back what is safe to write to the client.
    #[must_use]
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.carry.extend_from_slice(chunk);
        let valid = match std::str::from_utf8(&self.carry) {
            Ok(_) => self.carry.len(),
            Err(e) => e.valid_up_to(),
        };
        let tail = self.carry.split_off(valid);
        let text = String::from_utf8(std::mem::take(&mut self.carry)).unwrap_or_default();

        let hold = hold_from(&text);
        let out = self.dict.expand(&text[..hold]);
        self.carry = text.as_bytes()[hold..].to_vec();
        self.carry.extend_from_slice(&tail);
        out.into_bytes()
    }

    /// Whatever is still held back when the provider stops talking.
    #[must_use]
    pub fn finish(&mut self) -> Vec<u8> {
        let rest = std::mem::take(&mut self.carry);
        match String::from_utf8(rest) {
            Ok(text) => self.dict.expand(&text).into_bytes(),
            // Not valid UTF-8, so it holds no placeholder — the client gets it exactly as it came.
            Err(e) => e.into_bytes(),
        }
    }
}

/// Where a streamed answer keeps its text, in the order a frame is searched. The Anthropic deltas
/// first, then a chat-completion choice — a frame carries one of them, never all.
const TEXT_FIELDS: [&str; 3] = ["/delta/text", "/delta/partial_json", "/content_block/text"];

/// The same, inside one entry of a chat completion's `choices`.
const CHOICE_FIELDS: [&str; 2] = ["/delta/content", "/delta/reasoning_content"];

/// Expands placeholders in a server-sent event stream, frame by frame.
///
/// The text of each `data:` frame is pulled out, run through the same hold-and-expand as a whole
/// body, and written back — so a name split across two frames is completed by the second one rather
/// than reaching the client in halves. A frame whose text does not change is passed through exactly
/// as it arrived; only a frame that carries a placeholder is re-serialized, and that one may come
/// out with its keys in a different order, which is JSON's business and not a client's.
///
/// If a stream ends mid-name, the leftover is emitted as one more frame copied from the last one
/// that carried text — before the terminal frame, so a client that stops reading at `[DONE]` still
/// sees it.
#[derive(Debug)]
pub struct Frames {
    dict: Dict,
    /// A placeholder whose digits are still to come.
    carry: String,
    /// Bytes of a line the provider has not finished sending.
    line: Vec<u8>,
    /// The last frame that carried text, and where in it the text was, so a leftover has somewhere
    /// to go.
    template: Option<(Value, String)>,
}

impl Frames {
    /// Wrap a dictionary for frame-by-frame use.
    #[must_use]
    fn new(dict: Dict) -> Self {
        Self {
            dict,
            carry: String::new(),
            line: Vec::new(),
            template: None,
        }
    }

    /// Feed one chunk, get back the frames that are complete.
    #[must_use]
    fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.line.extend_from_slice(chunk);
        let mut out = Vec::new();
        // A frame is only actionable once its line is whole, so everything up to the last newline
        // is processed and the remainder waits.
        while let Some(at) = self.line.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.line.drain(..=at).collect();
            match std::str::from_utf8(&line) {
                Ok(text) => out.extend_from_slice(self.transform(text).as_bytes()),
                Err(_) => out.extend_from_slice(&line),
            }
        }
        out
    }

    /// Anything still buffered when the provider stops talking.
    #[must_use]
    fn finish(&mut self) -> Vec<u8> {
        let mut out = self.leftover().into_bytes();
        out.append(&mut self.line);
        out
    }

    /// One line of the stream, as it should reach the client.
    fn transform(&mut self, line: &str) -> String {
        let Some(payload) = line.strip_prefix("data:") else {
            return line.to_string();
        };
        let payload = payload.trim();
        if payload == "[DONE]" {
            // The last word: a held name has to be said before it, not after.
            return format!("{}{line}", self.leftover());
        }
        let Ok(mut frame) = serde_json::from_str::<Value>(payload) else {
            return line.to_string();
        };
        if is_terminal(&frame) {
            return format!("{}{line}", self.leftover());
        }
        if self.expand_frame(&mut frame) {
            let ending = line.strip_prefix(&format!("data:{payload}")).unwrap_or("\n");
            if let Ok(rendered) = serde_json::to_string(&frame) {
                return format!("data: {rendered}{ending}");
            }
        }
        line.to_string()
    }

    /// Expand every text field this frame has, reporting whether any of them moved.
    fn expand_frame(&mut self, frame: &mut Value) -> bool {
        let mut changed = false;
        for field in TEXT_FIELDS {
            changed |= self.expand_at(frame, field);
        }
        let choices = frame
            .get("choices")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        for i in 0..choices {
            for field in CHOICE_FIELDS {
                changed |= self.expand_at(frame, &format!("/choices/{i}{field}"));
            }
        }
        changed
    }

    /// Expand the string at one place in a frame, if there is one there.
    fn expand_at(&mut self, frame: &mut Value, pointer: &str) -> bool {
        let Some(text) = frame
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return false;
        };
        let expanded = self.feed(&text);
        // Kept even when nothing changed: this is where a leftover would have to go.
        self.template = Some((frame.clone(), pointer.to_string()));
        if expanded == text {
            return false;
        }
        if let Some(slot) = frame.pointer_mut(pointer) {
            *slot = Value::String(expanded);
        }
        true
    }

    /// Run text through the dictionary, holding back anything that could still become a name.
    fn feed(&mut self, text: &str) -> String {
        let mut buffer = std::mem::take(&mut self.carry);
        buffer.push_str(text);
        let hold = hold_from(&buffer);
        self.carry = buffer[hold..].to_string();
        self.dict.expand(&buffer[..hold])
    }

    /// A frame carrying whatever was still held, or nothing if nothing was.
    fn leftover(&mut self) -> String {
        let rest = std::mem::take(&mut self.carry);
        if rest.is_empty() {
            return String::new();
        }
        let expanded = self.dict.expand(&rest);
        let Some((frame, pointer)) = self.template.clone() else {
            return String::new();
        };
        let mut frame = frame;
        if let Some(slot) = frame.pointer_mut(&pointer) {
            *slot = Value::String(expanded);
        }
        serde_json::to_string(&frame).map_or_else(|_| String::new(), |f| format!("data: {f}\n\n"))
    }
}

/// Whether this frame is the end of the answer — the point by which everything held must be out.
fn is_terminal(frame: &Value) -> bool {
    matches!(
        frame.get("type").and_then(Value::as_str),
        Some("content_block_stop" | "message_delta" | "message_stop")
    ) || frame
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| {
            choices
                .iter()
                .any(|c| c.get("finish_reason").is_some_and(|r| !r.is_null()))
        })
}

/// The longest suffix of a chunk that could still turn into a placeholder, and so must wait for the
/// next one. The window is short because a name is a mark, or its six-character escape, plus digits.
fn hold_from(text: &str) -> usize {
    const WINDOW: usize = 12;
    let from = text
        .char_indices()
        .rev()
        .take(WINDOW)
        .last()
        .map_or(text.len(), |(i, _)| i);
    for (i, _) in text[from..].char_indices() {
        if could_start_placeholder(&text[from + i..]) {
            return from + i;
        }
    }
    text.len()
}

/// Whether this suffix is the beginning of a placeholder that has not finished arriving.
fn could_start_placeholder(tail: &str) -> bool {
    if let Some(digits) = tail.strip_prefix(MARK) {
        return digits.chars().all(|c| c.is_ascii_digit());
    }
    if let Some(digits) = tail.strip_prefix(MARK_ESCAPED) {
        return digits.chars().all(|c| c.is_ascii_digit());
    }
    !tail.is_empty() && MARK_ESCAPED.starts_with(tail)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    const P: &str = "/Users/someone/work/service/crates/webapp/src/pages/";

    fn body(times: usize) -> Vec<u8> {
        let listing = (0..times)
            .map(|i| format!("{P}file{i}.rs"))
            .collect::<Vec<_>>()
            .join("\n");
        serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": format!("read these\n{listing}")}],
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn a_literal_is_named_only_once_it_repeats_enough_to_pay_for_its_own_line() {
        assert!(Dict::build(&body(2)).is_empty());
        let dict = Dict::build(&body(8));
        assert_eq!(dict.len(), 1);
        assert!(dict.preamble(Mode::Full).contains(P));
        assert!(dict.estimated_saving(&body(8)) > 0);
    }

    #[test]
    fn shortening_and_expanding_are_exactly_inverse() {
        let dict = Dict::build(&body(8));
        let original = format!("look at {P}llm.rs and {P}db.rs");
        let short = dict.shorten(&original);
        assert!(!short.contains(P));
        assert_eq!(dict.expand(&short), original);
    }

    #[test]
    fn the_full_mode_rewrites_the_prompt_and_declares_what_it_did() {
        let dict = Dict::build(&body(8));
        let out = dict
            .apply(&body(8), Mode::Full, Shape::Anthropic)
            .expect("rewritten");
        let json: Value = serde_json::from_slice(&out).unwrap();
        let text = json["messages"][0]["content"].as_str().unwrap();
        assert!(!text.contains(P), "the prompt still spells the path out");
        assert!(text.contains("§1"));
        assert!(json["system"].as_str().unwrap().contains("§1 = "));
        // Matched by the provider against a list of its own, so never rewritten.
        assert_eq!(json["model"], "claude-haiku-4-5");
    }

    #[test]
    fn the_tail_mode_leaves_every_existing_byte_where_it_was() {
        let before: Value = serde_json::from_slice(&body(8)).unwrap();
        let dict = Dict::build(&body(8));

        let openai = dict
            .apply(&body(8), Mode::Tail, Shape::OpenAi)
            .expect("appended");
        let after: Value = serde_json::from_slice(&openai).unwrap();
        assert_eq!(after["messages"][0], before["messages"][0]);
        let added = after["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(added["role"], "system");
        assert!(added["content"].as_str().unwrap().contains("§1 = "));

        // Anthropic has no system role in the list, so the block joins the last user message.
        let anthropic = dict
            .apply(&body(8), Mode::Tail, Shape::Anthropic)
            .expect("appended");
        let after: Value = serde_json::from_slice(&anthropic).unwrap();
        assert_eq!(after["messages"].as_array().unwrap().len(), 1);
        let content = after["messages"][0]["content"].as_str().unwrap();
        assert!(content.starts_with("read these"), "the prompt moved");
        assert!(content.contains("§1 = "));
    }

    #[test]
    fn a_rewrite_that_would_not_pay_for_its_own_instructions_is_not_made() {
        // Three occurrences earn the literal a name, and still lose to the paragraph explaining it.
        let dict = Dict::build(&body(3));
        assert_eq!(dict.len(), 1);
        assert!(dict.estimated_saving(&body(3)) < prose_tokens(Mode::Full));
        assert!(dict.apply(&body(3), Mode::Full, Shape::OpenAi).is_none());
        // Tail is bought for what it does to the answer, so it is applied as asked.
        assert!(dict.apply(&body(3), Mode::Tail, Shape::OpenAi).is_some());
    }

    #[test]
    fn a_prefilled_answer_is_never_touched() {
        let dict = Dict::build(&body(8));
        let mut json: Value = serde_json::from_slice(&body(8)).unwrap();
        json["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"role": "assistant", "content": "Here is"}));
        let raw = serde_json::to_vec(&json).unwrap();
        assert!(dict.apply(&raw, Mode::Tail, Shape::Anthropic).is_none());
    }

    #[test]
    fn a_placeholder_split_across_two_chunks_still_comes_back_whole() {
        let mut expander = Expander::new(Dict::build(&body(8)));
        let mut out = expander.push(b"the file is ");
        // The mark itself is split down the middle of its two UTF-8 bytes.
        let mark = MARK.to_string().into_bytes();
        out.extend(expander.push(&mark[..1]));
        out.extend(expander.push(&mark[1..]));
        out.extend(expander.push(b"1llm.rs, there"));
        out.extend(expander.finish());
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("the file is {P}llm.rs, there")
        );
    }

    /// What a stream of one-token frames looks like, and what the client should end up reading.
    fn streamed(deltas: &[&str]) -> String {
        let mut wire = String::new();
        for delta in deltas {
            let frame = serde_json::json!({
                "choices": [{"index": 0, "delta": {"content": delta}}],
            });
            let _ = writeln!(wire, "data: {frame}\n");
        }
        wire.push_str("data: [DONE]\n\n");
        wire
    }

    /// The text a client would reassemble out of a stream.
    fn reassembled(wire: &str) -> String {
        wire.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter(|payload| *payload != "[DONE]")
            .filter_map(|payload| serde_json::from_str::<Value>(payload).ok())
            .filter_map(|frame| {
                frame
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect()
    }

    #[test]
    fn a_name_split_across_two_frames_is_still_expanded() {
        // This is how it really arrives: the mark is one token and the digit is the next, so the
        // two are a whole frame of JSON apart and never adjacent on the wire.
        let wire = streamed(&["the file ", "§", "1", "llm.rs", " is the one"]);
        let mut rewriter = Rewriter::new(Dict::build(&body(8)), true);
        let mut out = rewriter.push(wire.as_bytes());
        out.extend(rewriter.finish());
        let client = String::from_utf8(out).unwrap();
        assert_eq!(
            reassembled(&client),
            format!("the file {P}llm.rs is the one")
        );
        assert!(client.ends_with("data: [DONE]\n\n"), "the stream lost its end");
    }

    #[test]
    fn a_stream_that_ends_mid_name_still_says_what_it_meant() {
        let wire = streamed(&["see ", "§", "1"]);
        let mut rewriter = Rewriter::new(Dict::build(&body(8)), true);
        let mut out = rewriter.push(wire.as_bytes());
        out.extend(rewriter.finish());
        let client = String::from_utf8(out).unwrap();
        assert_eq!(reassembled(&client), format!("see {P}"));
    }

    #[test]
    fn a_stream_arriving_in_arbitrary_byte_sized_pieces_reads_the_same() {
        let wire = streamed(&["a ", "§", "1", "db.rs", "!"]);
        let mut rewriter = Rewriter::new(Dict::build(&body(8)), true);
        let mut out = Vec::new();
        for byte in wire.as_bytes() {
            out.extend(rewriter.push(std::slice::from_ref(byte)));
        }
        out.extend(rewriter.finish());
        assert_eq!(
            reassembled(&String::from_utf8(out).unwrap()),
            format!("a {P}db.rs!")
        );
    }

    #[test]
    fn a_frame_carrying_no_placeholder_is_passed_through_untouched() {
        let wire = streamed(&["nothing", " to ", "do"]);
        let mut rewriter = Rewriter::new(Dict::build(&body(8)), true);
        let mut out = rewriter.push(wire.as_bytes());
        out.extend(rewriter.finish());
        assert_eq!(String::from_utf8(out).unwrap(), wire);
    }

    #[test]
    fn an_escaped_name_expands_the_same_as_a_literal_one() {
        let dict = Dict::build(&body(8));
        let mut expander = Expander::new(dict);
        let mut out = expander.push(br#"{"text":"see \u00a"#);
        out.extend(expander.push(br#"71llm.rs"}"#));
        out.extend(expander.finish());
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(r#"{{"text":"see {P}llm.rs"}}"#)
        );
    }

    #[test]
    fn a_double_digit_name_is_not_eaten_by_a_single_digit_one() {
        let dict = Dict {
            entries: vec![
                ("§1".to_string(), "/one/two/three/four".to_string()),
                ("§12".to_string(), "/ten/eleven/twelve/x".to_string()),
            ],
        };
        assert_eq!(dict.expand("§12"), "/ten/eleven/twelve/x");
        // And the same for the escaped spelling a JSON encoder may use.
        assert_eq!(dict.expand(r"\u00a712"), "/ten/eleven/twelve/x");
    }

    #[test]
    fn text_with_nothing_repeated_is_left_alone_entirely() {
        let plain = serde_json::json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "what is the capital of France"}],
        })
        .to_string();
        let dict = Dict::build(plain.as_bytes());
        assert!(dict.is_empty());
        assert!(
            dict.apply(plain.as_bytes(), Mode::Full, Shape::OpenAi)
                .is_none()
        );
    }

    #[test]
    fn a_provider_whose_body_shape_is_unknown_gets_no_injection() {
        assert_eq!(Shape::for_provider("anthropic"), Some(Shape::Anthropic));
        assert_eq!(Shape::for_provider("zai"), Some(Shape::OpenAi));
        assert!(Shape::for_provider("gemini").is_none());
    }

    #[test]
    fn only_the_modes_that_exist_are_accepted() {
        assert_eq!(Mode::parse("full"), Some(Mode::Full));
        assert_eq!(Mode::parse(" Tail "), Some(Mode::Tail));
        assert!(Mode::parse("off").is_none());
        assert!(Mode::parse("").is_none());
    }
}
