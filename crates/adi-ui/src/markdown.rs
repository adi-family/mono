//! [`Markdown`] — the rendered half of a `.md` file, and of anything an agent says.
//!
//! A small subset, scanned rather than parsed: headings, fenced code, lists, quotes, rules,
//! GitHub-style tables, paragraphs, and inline `code` / `**strong**` / `*em*` /
//! `[links](url)`. Like [`crate::highlight`] it is **total** — no input is invalid, an
//! unterminated fence runs to the end, and anything it does not recognise stays text.
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
    /// A header row, the body under it, and one alignment per column.
    Table {
        head: Vec<String>,
        rows: Vec<Vec<String>>,
        aligns: Vec<Align>,
    },
    Para(String),
}

/// Which way a column leans, as its delimiter row asked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Align {
    /// Nothing declared — which reads left, like the prose around it.
    Default,
    Left,
    Center,
    Right,
}

impl Align {
    /// The utility that does it, written out per arm because Tailwind only finds what it
    /// can read as a literal.
    fn class(self) -> &'static str {
        match self {
            Align::Default | Align::Left => "text-left",
            Align::Center => "text-center",
            Align::Right => "text-right",
        }
    }
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
        } else if let Some(aligns) = table_at(&lines, i) {
            // The header and the delimiter row under it are both spoken for; everything
            // after is a body row for as long as the rows keep coming.
            let head = split_row(line);
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() {
                let l = lines[i].trim();
                if l.is_empty() || !l.contains('|') {
                    break;
                }
                rows.push(split_row(l));
                i += 1;
            }
            out.push(Block::Table { head, rows, aligns });
        } else {
            let mut text = String::new();
            while i < lines.len() {
                if lines[i].trim().is_empty() || opens_block(&lines, i) {
                    break;
                }
                push_soft(&mut text, lines[i].trim());
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

/// Whether a block of its own starts at `lines[i]`, and so ends the paragraph above it.
///
/// It takes the whole document rather than the one line because a table is the one block
/// that cannot be recognised from its first line: `a | b` is a sentence until the row of
/// dashes under it says otherwise.
fn opens_block(lines: &[&str], i: usize) -> bool {
    let line = lines[i].trim();
    line.starts_with("```")
        || line.starts_with('>')
        || heading(line).is_some()
        || is_rule(line)
        || list_item(line).is_some()
        || table_at(lines, i).is_some()
}

/// The column alignments of a table starting at `lines[i]`, or `None` for anything that is
/// not one.
///
/// Three things have to hold, and each rules out a different false positive: the first line
/// has cells, the second is a delimiter row, and the two agree on how many columns there
/// are. That last one is what keeps `a | b` over a `---` — a sentence above a horizontal
/// rule — from being read as a one-column table.
fn table_at(lines: &[&str], i: usize) -> Option<Vec<Align>> {
    if !lines[i].contains('|') {
        return None;
    }
    let aligns = table_delimiter(lines.get(i + 1)?)?;
    (aligns.len() == split_row(lines[i]).len()).then_some(aligns)
}

/// `| --- | :--: | ---: |` → one [`Align`] per column. Every cell has to be a run of dashes,
/// with a colon on whichever side the column leans towards.
fn table_delimiter(line: &str) -> Option<Vec<Align>> {
    let cells = split_row(line);
    if cells.is_empty() {
        return None;
    }
    cells
        .iter()
        .map(|cell| {
            let cell = cell.trim();
            let rule = cell.trim_matches(':');
            let dashed = !rule.is_empty() && rule.bytes().all(|b| b == b'-');
            dashed.then(|| match (cell.starts_with(':'), cell.ends_with(':')) {
                (true, true) => Align::Center,
                (true, false) => Align::Left,
                (false, true) => Align::Right,
                (false, false) => Align::Default,
            })
        })
        .collect()
}

/// A row's cells: split on `|`, with `\|` kept as the literal pipe it is escaping, and the
/// empty cell an optional leading or trailing `|` leaves behind dropped at each end.
fn split_row(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = line.trim().chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    cells.push(cur);
    if cells.first().is_some_and(|c| c.trim().is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|c| c.trim().is_empty()) {
        cells.pop();
    }
    cells
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
        // A table is a *thing*, so it takes the island shape the rest of the system does —
        // and `shrink-0` beside `overflow-x-auto` for exactly the reason the code block
        // above spells out: a scroll container is the one child a flex column can squeeze
        // to nothing. `w-full` lets a narrow table fill the message; a wide one outgrows it
        // and scrolls inside its own edges rather than stretching the bubble.
        Block::Table { head, rows, aligns } => {
            let align = |col: usize| aligns.get(col).copied().unwrap_or(Align::Default).class();
            let head = head
                .iter()
                .enumerate()
                .map(|(col, cell)| {
                    let class = format!(
                        "border-b border-edge px-3 py-1.5 font-semibold text-ink {}",
                        align(col),
                    );
                    view! { <th class=class>{inline(cell.trim())}</th> }
                })
                .collect::<Vec<_>>();
            let body = rows
                .iter()
                .map(|row| {
                    let cells = row
                        .iter()
                        .enumerate()
                        .map(|(col, cell)| {
                            let class = format!(
                                "border-t border-divider px-3 py-1.5 align-top {}",
                                align(col),
                            );
                            view! { <td class=class>{inline(cell.trim())}</td> }
                        })
                        .collect::<Vec<_>>();
                    view! { <tr>{cells}</tr> }
                })
                .collect::<Vec<_>>();
            view! {
                <div class="island shrink-0 overflow-x-auto">
                    <table class="w-full border-collapse">
                        <thead class="bg-card">
                            <tr>{head}</tr>
                        </thead>
                        <tbody>{body}</tbody>
                    </table>
                </div>
            }
            .into_any()
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The block scan, as a shape that is easy to assert on — rendering a [`Block`] needs a
    /// reactive runtime, and none of what is worth testing here happens on that side.
    fn shapes(src: &str) -> Vec<String> {
        blocks(src)
            .into_iter()
            .map(|b| match b {
                Block::Heading(level, text) => format!("h{level}:{text}"),
                Block::Code(_, text) => format!("code:{}", text.trim_end()),
                Block::List { ordered, items } => {
                    format!("{}:{}", if ordered { "ol" } else { "ul" }, items.join(","))
                }
                Block::Quote(text) => format!("quote:{text}"),
                Block::Rule => "rule".to_string(),
                Block::Table { head, rows, .. } => {
                    let rows: Vec<String> = rows.iter().map(|r| r.join("|")).collect();
                    format!("table:[{}]{}", head.join("|"), rows.join("/"))
                }
                Block::Para(text) => format!("p:{text}"),
            })
            .collect()
    }

    #[test]
    fn a_table_is_a_table() {
        assert_eq!(
            shapes("| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |"),
            ["table:[ a | b ] 1 | 2 / 3 | 4 "],
        );
    }

    #[test]
    fn the_outer_pipes_are_optional() {
        assert_eq!(shapes("a | b\n--- | ---\n1 | 2"), ["table:[a | b]1 | 2"]);
    }

    #[test]
    fn colons_set_the_alignment() {
        let aligns = table_delimiter("| :-- | :-: | --: | --- |").unwrap();
        assert_eq!(
            aligns,
            [Align::Left, Align::Center, Align::Right, Align::Default],
        );
        assert_eq!(
            aligns.iter().map(|a| a.class()).collect::<Vec<_>>(),
            ["text-left", "text-center", "text-right", "text-left"],
        );
    }

    #[test]
    fn an_escaped_pipe_stays_in_its_cell() {
        assert_eq!(split_row(r"| a \| b | c |"), [" a | b ", " c "]);
    }

    /// The three ways a line with a pipe in it is *not* a table. The last is the one the
    /// column count is there for: without it, a sentence sitting above a horizontal rule
    /// reads as a one-column table.
    #[test]
    fn a_pipe_alone_is_not_a_table() {
        assert_eq!(shapes("a | b\nc | d"), ["p:a | b c | d"]);
        assert_eq!(shapes("a | b\n| -x- | -y- |"), ["p:a | b | -x- | -y- |"]);
        assert_eq!(shapes("a | b\n---"), ["p:a | b", "rule"]);
    }

    /// A delimiter row is dashes, and a run of dashes is a rule — so a table that lost its
    /// header has to come out as the two lines it is, not as a table with no columns.
    #[test]
    fn a_headless_delimiter_is_a_rule() {
        assert_eq!(shapes("| --- |\n| 1 |"), ["p:| --- | | 1 |"]);
    }

    #[test]
    fn a_table_ends_the_paragraph_above_it() {
        assert_eq!(
            shapes("intro\n\n| a |\n| - |\n| 1 |\n\nafter"),
            ["p:intro", "table:[ a ] 1 ", "p:after"],
        );
        // …and without the blank line, which is where `opens_block` earns its lookahead.
        assert_eq!(
            shapes("intro\n| a |\n| - |\n| 1 |"),
            ["p:intro", "table:[ a ] 1 "],
        );
    }

    #[test]
    fn a_ragged_row_is_kept_as_written() {
        assert_eq!(
            shapes("| a | b |\n| - | - |\n| 1 |\n| 1 | 2 | 3 |"),
            ["table:[ a | b ] 1 / 1 | 2 | 3 "],
        );
    }

    /// A list wins the tie, because `- a | b` over `- | -` is two bullets far more often
    /// than it is a table.
    #[test]
    fn a_list_is_not_swallowed_by_a_table() {
        assert_eq!(shapes("- a | b\n- | -"), ["ul:a | b,| -"]);
    }

    #[test]
    fn the_blocks_around_a_table_still_work() {
        assert_eq!(
            shapes("# T\n\n- one\n\n> q\n\n```rs\nlet x = 1;\n```"),
            ["h1:T", "ul:one", "quote:q", "code:let x = 1;"],
        );
    }
}
