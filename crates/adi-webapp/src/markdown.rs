//! A small, safe Markdown renderer for chat messages.
//!
//! Hand-rolled rather than pulled from a crate — the same reasoning as [`crate::highlight`]: a
//! Markdown crate (pulldown-cmark, comrak) would add real weight to the wasm bundle, and agent
//! messages use a narrow slice of Markdown. This covers that slice: fenced code blocks, ATX
//! headings, bullet / numbered lists, blockquotes, GitHub-style tables, paragraphs, and inline
//! **bold**, *italic*, `code`, and [links](https://example.com).
//!
//! It builds Leptos elements directly — never `inner_html` — so every run of text is escaped by the
//! framework and the renderer cannot inject markup. Link URLs are scheme-checked (`http`/`https`/
//! `mailto` or relative only), so a `javascript:` href can't slip through. Unknown or malformed
//! syntax degrades to plain text; every function is total and never panics.

use leptos::prelude::*;

/// Render `src` as Markdown into a `<div class="adi-md">`.
pub(crate) fn render(src: &str) -> AnyView {
    let blocks = parse_blocks(src);
    view! { <div class="adi-md">{blocks}</div> }.into_any()
}

/// Block-level parse: walk the lines, grouping them into headings, fenced code, lists, blockquotes,
/// tables, and paragraphs. A blank line separates blocks.
fn parse_blocks(src: &str) -> Vec<AnyView> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out: Vec<AnyView> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // Fenced code block: ``` or ~~~ (a language after the opening fence is ignored). Collected
        // verbatim until a matching closing fence (or end of input).
        if let Some(fence) = fence_char(trimmed) {
            let mut code = String::new();
            i += 1;
            while i < lines.len() && fence_char(lines[i].trim_start()) != Some(fence) {
                code.push_str(lines[i]);
                code.push('\n');
                i += 1;
            }
            if i < lines.len() {
                i += 1; // consume the closing fence
            }
            out.push(view! { <pre class="adi-md__pre"><code>{code}</code></pre> }.into_any());
            continue;
        }

        // ATX heading: 1–6 leading `#` then a space.
        if let Some((level, rest)) = heading(trimmed) {
            let class = format!("adi-md__h adi-md__h{level}");
            out.push(view! { <div class=class>{parse_inline(rest)}</div> }.into_any());
            i += 1;
            continue;
        }

        // Blockquote: consecutive `>`-prefixed lines.
        if trimmed.starts_with('>') {
            let mut quoted = String::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let l = lines[i].trim_start().strip_prefix('>').unwrap_or_default();
                let l = l.strip_prefix(' ').unwrap_or(l);
                if !quoted.is_empty() {
                    quoted.push(' ');
                }
                quoted.push_str(l);
                i += 1;
            }
            out.push(view! { <blockquote class="adi-md__quote">{parse_inline(&quoted)}</blockquote> }.into_any());
            continue;
        }

        // List: consecutive bullet (`-`/`*`/`+`) or numbered (`1.`/`1)`) items of the same kind.
        if list_item(trimmed).is_some() {
            let ordered = numbered(trimmed);
            let mut items: Vec<AnyView> = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim_start();
                let Some(content) = list_item(t) else { break };
                if numbered(t) != ordered {
                    break; // don't let a bullet list swallow a numbered one (or vice versa)
                }
                items.push(view! { <li>{parse_inline(content)}</li> }.into_any());
                i += 1;
            }
            out.push(if ordered {
                view! { <ol class="adi-md__list">{items}</ol> }.into_any()
            } else {
                view! { <ul class="adi-md__list">{items}</ul> }.into_any()
            });
            continue;
        }

        // Table (GitHub-flavored): a header row of `|`-separated cells followed by a delimiter row
        // (`| --- | :--: |`). Requiring the delimiter on the very next line keeps a paragraph that
        // merely contains a `|` from being mistaken for a table.
        if trimmed.contains('|')
            && i + 1 < lines.len()
            && let Some(aligns) = table_delimiter(lines[i + 1].trim())
        {
            let header = split_table_row(trimmed);
            i += 2;
            let mut rows: Vec<Vec<String>> = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                if t.is_empty() || !t.contains('|') {
                    break;
                }
                rows.push(split_table_row(t));
                i += 1;
            }
            out.push(render_table(&header, &rows, &aligns));
            continue;
        }

        // Paragraph: consecutive lines until a blank line or a block-starter. Soft line breaks
        // become spaces (standard Markdown).
        let mut para = String::new();
        while i < lines.len() {
            let t = lines[i].trim_start();
            if t.is_empty()
                || fence_char(t).is_some()
                || heading(t).is_some()
                || t.starts_with('>')
                || list_item(t).is_some()
                || (t.contains('|')
                    && i + 1 < lines.len()
                    && table_delimiter(lines[i + 1].trim()).is_some())
            {
                break;
            }
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(lines[i].trim());
            i += 1;
        }
        out.push(view! { <p class="adi-md__p">{parse_inline(&para)}</p> }.into_any());
    }
    out
}

/// Per-column text alignment declared by a table's delimiter row.
#[derive(Clone, Copy)]
enum Align {
    None,
    Left,
    Center,
    Right,
}

impl Align {
    /// The inline `style` value for a cell (empty for the default alignment).
    fn style(self) -> &'static str {
        match self {
            Align::None => "",
            Align::Left => "text-align:left",
            Align::Center => "text-align:center",
            Align::Right => "text-align:right",
        }
    }
}

/// If `line` is a table delimiter row (`| --- | :--: | ---: |`), return the per-column alignment;
/// else `None`. Every cell must be a run of dashes with optional leading/trailing colons.
fn table_delimiter(line: &str) -> Option<Vec<Align>> {
    let cells = split_table_row(line);
    if cells.is_empty() {
        return None;
    }
    let mut aligns = Vec::with_capacity(cells.len());
    for cell in &cells {
        let c = cell.trim();
        let left = c.starts_with(':');
        let right = c.ends_with(':');
        let dashes = c.trim_matches(':');
        if dashes.is_empty() || !dashes.bytes().all(|b| b == b'-') {
            return None;
        }
        aligns.push(match (left, right) {
            (true, true) => Align::Center,
            (true, false) => Align::Left,
            (false, true) => Align::Right,
            (false, false) => Align::None,
        });
    }
    Some(aligns)
}

/// Split a table row into its cells: break on unescaped `|`, unescape `\|`, and drop the empty edge
/// cells produced by an optional leading / trailing `|`.
fn split_table_row(line: &str) -> Vec<String> {
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

/// Render a parsed table: a header row, the body rows, and per-column alignment. A body row with
/// more cells than the header still renders them; a shorter row just ends early.
fn render_table(header: &[String], rows: &[Vec<String>], aligns: &[Align]) -> AnyView {
    let style = |col: usize| aligns.get(col).copied().unwrap_or(Align::None).style();
    let head: Vec<AnyView> = header
        .iter()
        .enumerate()
        .map(|(c, cell)| view! { <th style=style(c)>{parse_inline(cell.trim())}</th> }.into_any())
        .collect();
    let body: Vec<AnyView> = rows
        .iter()
        .map(|row| {
            let cells: Vec<AnyView> = row
                .iter()
                .enumerate()
                .map(|(c, cell)| {
                    view! { <td style=style(c)>{parse_inline(cell.trim())}</td> }.into_any()
                })
                .collect();
            view! { <tr>{cells}</tr> }.into_any()
        })
        .collect();
    // Wrapped in a scroll container so a table only ever scrolls (never widens the message bubble)
    // when its content genuinely needs more room than the message is wide.
    view! {
        <div class="adi-md__table-wrap">
            <table class="adi-md__table">
                <thead><tr>{head}</tr></thead>
                <tbody>{body}</tbody>
            </table>
        </div>
    }
    .into_any()
}

/// The fence character (`` ` `` or `~`) if `line` opens a fenced code block, else `None`.
fn fence_char(line: &str) -> Option<char> {
    if line.starts_with("```") {
        Some('`')
    } else if line.starts_with("~~~") {
        Some('~')
    } else {
        None
    }
}

/// An ATX heading's level (1–6) and the text after the `#`s, or `None`.
fn heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) {
        if let Some(text) = line[hashes..].strip_prefix(' ') {
            return Some((hashes, text.trim_end()));
        }
    }
    None
}

/// Whether `line` starts a numbered list item (`1.` / `1)`).
fn numbered(line: &str) -> bool {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return false;
    }
    let after = &line[digits..];
    (after.starts_with('.') || after.starts_with(')')) && after[1..].starts_with(' ')
}

/// The content of a list item (bullet or numbered) with its marker stripped, or `None`.
fn list_item(line: &str) -> Option<&str> {
    if let Some(rest) = line
        .strip_prefix("- ")
        .or(line.strip_prefix("* "))
        .or(line.strip_prefix("+ "))
    {
        return Some(rest);
    }
    if numbered(line) {
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        return Some(line[digits + 1..].trim_start());
    }
    None
}

/// Inline parse: split `text` into a run of views handling code spans, links, bold, and italic.
/// Precedence is code (suppresses everything inside) → link → bold → italic; anything unmatched is
/// plain, escaped text.
fn parse_inline(text: &str) -> Vec<AnyView> {
    let mut out: Vec<AnyView> = Vec::new();
    let mut buf = String::new();
    let mut rest = text;

    while !rest.is_empty() {
        // Inline code span: `code` — highest precedence, so markers inside stay literal.
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
        {
            flush(&mut buf, &mut out);
            out.push(view! { <code class="adi-md__code">{after[..end].to_string()}</code> }.into_any());
            rest = &after[end + 1..];
            continue;
        }

        // Link: [label](url).
        if rest.starts_with('[')
            && let Some((label, url, consumed)) = parse_link(rest)
        {
            flush(&mut buf, &mut out);
            let children = parse_inline(label);
            match sanitize_url(url) {
                Some(href) => out.push(
                    view! {
                        <a href=href target="_blank" rel="noopener noreferrer" class="adi-md__link">
                            {children}
                        </a>
                    }
                    .into_any(),
                ),
                // Drop an unsafe URL but keep the label text, so nothing is silently swallowed.
                None => out.extend(children),
            }
            rest = &rest[consumed..];
            continue;
        }

        // Bold: **text** or __text__ (before italic, so `**` isn't read as two `*`).
        if let Some((inner, after)) = match_wrap(rest, "**").or_else(|| match_wrap(rest, "__")) {
            flush(&mut buf, &mut out);
            out.push(view! { <strong>{parse_inline(inner)}</strong> }.into_any());
            rest = after;
            continue;
        }

        // Italic: *text* or _text_.
        if let Some((inner, after)) = match_wrap(rest, "*").or_else(|| match_wrap(rest, "_")) {
            flush(&mut buf, &mut out);
            out.push(view! { <em>{parse_inline(inner)}</em> }.into_any());
            rest = after;
            continue;
        }

        // Plain: consume one char.
        let ch = rest.chars().next().unwrap();
        buf.push(ch);
        rest = &rest[ch.len_utf8()..];
    }

    flush(&mut buf, &mut out);
    out
}

/// If `rest` opens with `delim`, find the matching closing `delim` and return `(inner, after)` —
/// the text between the delimiters and the remainder past the close. A zero-length inner (an empty
/// `**` / `**`) is rejected so the markers stay literal.
fn match_wrap<'a>(rest: &'a str, delim: &str) -> Option<(&'a str, &'a str)> {
    let after_open = rest.strip_prefix(delim)?;
    let end = after_open.find(delim)?;
    if end == 0 {
        return None;
    }
    Some((&after_open[..end], &after_open[end + delim.len()..]))
}

/// Emit the accumulated plain text (if any) as an escaped text node.
fn flush(buf: &mut String, out: &mut Vec<AnyView>) {
    if !buf.is_empty() {
        let text = std::mem::take(buf);
        out.push(view! { {text} }.into_any());
    }
}

/// Parse a `[label](url)` link at the start of `rest`, returning `(label, url, bytes_consumed)`.
fn parse_link(rest: &str) -> Option<(&str, &str, usize)> {
    let close_label = rest.find(']')?;
    let after = &rest[close_label + 1..];
    let inner = after.strip_prefix('(')?;
    let close_url = inner.find(')')?;
    let label = &rest[1..close_label];
    let url = &inner[..close_url];
    let consumed = close_label + 1 + 1 + close_url + 1;
    Some((label, url, consumed))
}

/// Allow only clearly safe link targets: `http`/`https`/`mailto` URLs, or a relative path / anchor
/// (no scheme). Everything else — notably `javascript:` and `data:` — is rejected.
fn sanitize_url(url: &str) -> Option<String> {
    let u = url.trim();
    if u.is_empty() {
        return None;
    }
    let lower = u.to_ascii_lowercase();
    let safe = lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || u.starts_with('/')
        || u.starts_with('#')
        || !u.contains(':'); // a relative path with no scheme at all
    safe.then(|| u.to_string())
}
