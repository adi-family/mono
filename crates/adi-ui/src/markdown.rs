//! [`Markdown`] — the rendered half of a `.md` file, and of anything an agent says.
//!
//! A small subset, scanned rather than parsed: headings, fenced code, lists, quotes, rules,
//! paragraphs, and inline `code` / `**strong**` / `*em*` / `[links](url)`. Like
//! [`crate::highlight`] it is **total** — no input is invalid, an unterminated fence runs to
//! the end, and anything it does not recognise stays text.
//!
//! It renders through Leptos views, never `inner_html`, so a document cannot inject markup
//! however it is written. Link targets are checked separately (see `safe_href`) because a
//! URL is the one thing here that becomes a live capability.

use leptos::prelude::*;

use crate::{
    highlight::{Lang, highlight},
    merge,
};

/// A document, rendered.
///
/// ```ignore
/// <Markdown source=Signal::derive(move || buffer.get())/>
/// ```
#[component]
pub fn Markdown(
    #[prop(into)] source: Signal<String>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <div class=merge("flex flex-col gap-3 text-msg text-body", class)>
            {move || blocks(&source.get()).into_iter().map(block).collect::<Vec<_>>()}
        </div>
    }
}

/// One block of a document.
enum Block {
    /// Level 1-6, already stripped of its hashes.
    Heading(usize, String),
    Code(Lang, String),
    List { ordered: bool, items: Vec<String> },
    Quote(String),
    Rule,
    Para(String),
}

/// Split a document into blocks. Line-based, single pass, and it never fails: whatever it
/// cannot classify becomes a paragraph.
fn blocks(src: &str) -> Vec<Block> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim_start();

        if line.is_empty() {
            i += 1;
        } else if let Some(info) = line.strip_prefix("```") {
            // An info string is a file extension in disguise, which is exactly what
            // `Lang::from_path` reads.
            let lang = Lang::from_path(&format!("f.{}", info.trim()));
            let mut text = String::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                text.push_str(lines[i]);
                text.push('\n');
                i += 1;
            }
            // Past the closing fence — or past the end, for a fence nobody closed.
            i += 1;
            out.push(Block::Code(lang, text));
        } else if let Some((level, text)) = heading(line) {
            out.push(Block::Heading(level, text));
            i += 1;
        } else if is_rule(line) {
            out.push(Block::Rule);
            i += 1;
        } else if line.starts_with('>') {
            let mut text = String::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let l = lines[i].trim_start().trim_start_matches('>').trim();
                push_soft(&mut text, l);
                i += 1;
            }
            out.push(Block::Quote(text));
        } else if let Some((ordered, first)) = list_item(line) {
            let mut items = vec![first];
            i += 1;
            while i < lines.len() {
                match list_item(lines[i].trim_start()) {
                    Some((same, item)) if same == ordered => {
                        items.push(item);
                        i += 1;
                    }
                    _ => break,
                }
            }
            out.push(Block::List { ordered, items });
        } else {
            let mut text = String::new();
            while i < lines.len() {
                let l = lines[i].trim();
                if l.is_empty() || opens_block(l) {
                    break;
                }
                push_soft(&mut text, l);
                i += 1;
            }
            out.push(Block::Para(text));
        }
    }
    out
}

/// Append a line to a running block, joining a soft wrap with a space the way Markdown does.
fn push_soft(buf: &mut String, line: &str) {
    if !buf.is_empty() {
        buf.push(' ');
    }
    buf.push_str(line);
}

/// Whether this line would start a block of its own, and so ends the paragraph above it.
fn opens_block(line: &str) -> bool {
    line.starts_with("```")
        || line.starts_with('>')
        || heading(line).is_some()
        || is_rule(line)
        || list_item(line).is_some()
}

/// `## Title` → `(2, "Title")`. Requires the space: `#hashtag` is a word.
fn heading(line: &str) -> Option<(usize, String)> {
    let level = line.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&level) && line[level..].starts_with(' ') {
        Some((level, line[level..].trim().to_string()))
    } else {
        None
    }
}

/// Three or more of `-`, `*` or `_`, and nothing else.
fn is_rule(line: &str) -> bool {
    let bare: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    bare.len() >= 3
        && (bare.chars().all(|c| c == '-')
            || bare.chars().all(|c| c == '*')
            || bare.chars().all(|c| c == '_'))
}

/// `- item` / `* item` / `+ item` → unordered; `1. item` → ordered.
fn list_item(line: &str) -> Option<(bool, String)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some((false, rest.trim().to_string()));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && let Some(rest) = line[digits..].strip_prefix(". ")
    {
        return Some((true, rest.trim().to_string()));
    }
    None
}

/// One block, as a view. Every class is written out per arm, because Tailwind only finds
/// what it can read as a literal.
fn block(b: Block) -> AnyView {
    match b {
        Block::Heading(1, text) => {
            view! { <h1 class="m-0 text-title font-semibold text-ink">{inline(&text)}</h1> }
                .into_any()
        }
        Block::Heading(2, text) => {
            view! { <h2 class="m-0 text-sub font-semibold text-ink">{inline(&text)}</h2> }
                .into_any()
        }
        Block::Heading(_, text) => {
            view! { <h3 class="m-0 text-msg font-semibold text-ink">{inline(&text)}</h3> }
                .into_any()
        }
        // `shrink-0` is load-bearing, not decoration. `overflow-x-auto` makes this a scroll
        // container, and a scroll container's automatic minimum size is 0 rather than its
        // content — so inside the flex column above, a code block was the one block that
        // could be squeezed, and it was: the last line came out sliced in half.
        //
        // The fill is `bubble`, the surface that sits *above* a card in both themes. `stage`
        // is below it, and a block of code that reads as a hole in the page is a block
        // nobody's eye stops on.
        Block::Code(lang, text) => view! {
            <pre class="m-0 shrink-0 overflow-x-auto rounded-sm border border-edge bg-bubble \
                        p-3 font-mono text-mini leading-[1.55] [tab-size:2] text-syn-plain">
                {highlight(lang, &text)
                    .into_iter()
                    .map(|(tok, run)| view! { <span class=tok.classes()>{run}</span> })
                    .collect::<Vec<_>>()}
            </pre>
        }
        .into_any(),
        Block::List { ordered, items } => {
            let rows = items
                .into_iter()
                .map(|item| view! { <li class="mt-1 pl-1 first:mt-0">{inline(&item)}</li> })
                .collect::<Vec<_>>();
            if ordered {
                view! { <ol class="m-0 list-decimal pl-5">{rows}</ol> }.into_any()
            } else {
                view! { <ul class="m-0 list-disc pl-5">{rows}</ul> }.into_any()
            }
        }
        Block::Quote(text) => view! {
            <blockquote class="m-0 border-l-2 border-dim pl-3 text-secondary">
                {inline(&text)}
            </blockquote>
        }
        .into_any(),
        Block::Rule => view! { <hr class="m-0 h-px border-0 bg-divider"/> }.into_any(),
        Block::Para(text) => view! { <p class="m-0">{inline(&text)}</p> }.into_any(),
    }
}

/// Inline spans: `code`, `**strong**`, `*em*`, `[text](url)`. Anything unmatched — a lone
/// asterisk, an unclosed bracket — stays the character it is.
fn inline(src: &str) -> Vec<AnyView> {
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<AnyView> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;

    while i < chars.len() {
        let matched = match chars[i] {
            '`' => delimited(&chars, i, "`").map(|(text, next)| {
                let v = view! {
                    <code class="rounded-sm bg-bubble px-1 py-0.5 font-mono text-mini text-ink">
                        {text}
                    </code>
                }
                .into_any();
                (v, next)
            }),
            '*' if chars.get(i + 1) == Some(&'*') => delimited(&chars, i, "**")
                .map(|(text, next)| (view! { <strong>{text}</strong> }.into_any(), next)),
            '*' | '_' => {
                let d = chars[i].to_string();
                delimited(&chars, i, &d)
                    .map(|(text, next)| (view! { <em>{text}</em> }.into_any(), next))
            }
            '[' => link(&chars, i),
            _ => None,
        };

        if let Some((node, next)) = matched {
            if !plain.is_empty() {
                out.push(view! { <span>{std::mem::take(&mut plain)}</span> }.into_any());
            }
            out.push(node);
            i = next;
        } else {
            plain.push(chars[i]);
            i += 1;
        }
    }

    if !plain.is_empty() {
        out.push(view! { <span>{plain}</span> }.into_any());
    }
    out
}

/// The text between `delim` at `i` and the next `delim`, and the index past the closing one.
/// `None` when it never closes, which leaves the opener as ordinary text.
fn delimited(chars: &[char], i: usize, delim: &str) -> Option<(String, usize)> {
    let d: Vec<char> = delim.chars().collect();
    let start = i + d.len();
    let mut j = start;
    while j + d.len() <= chars.len() {
        if chars[j..j + d.len()] == d[..] {
            let text: String = chars[start..j].iter().collect();
            return (!text.is_empty()).then_some((text, j + d.len()));
        }
        j += 1;
    }
    None
}

/// `[text](url)` — both halves, or nothing.
fn link(chars: &[char], i: usize) -> Option<(AnyView, usize)> {
    let close = (i + 1..chars.len()).find(|&j| chars[j] == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = (close + 2..chars.len()).find(|&j| chars[j] == ')')?;
    let text: String = chars[i + 1..close].iter().collect();
    let url: String = chars[close + 2..end].iter().collect();
    let href = safe_href(&url)?;
    Some((
        view! { <a href=href rel="noreferrer noopener">{text}</a> }.into_any(),
        end + 1,
    ))
}

/// A link target, if it is one worth handing to a browser.
///
/// The web has one scheme that turns a document into code — `javascript:` — and a document
/// here can come from an agent, a repository, or anyone who can write a file. So this is an
/// allow-list rather than a block-list: http, https, mailto, or a same-document/relative
/// target. Everything else renders as text, which is what it always was.
fn safe_href(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    // No scheme at all — `docs/indexer.md`. A `:` only means a scheme when it comes before
    // the first `/`, which is what keeps `./a:b/c` a path.
    let relative = lower
        .split_once(':')
        .is_none_or(|(scheme, _)| scheme.contains('/'));

    let ok = lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || trimmed.starts_with('/')
        || trimmed.starts_with('#')
        || relative;
    ok.then(|| trimmed.to_string())
}
