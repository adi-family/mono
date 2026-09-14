//! The publisher's long form, rendered to HTML.
//!
//! The same subset `adi_ui::Markdown` draws in the control panel — headings, fenced code, lists,
//! quotes, rules, pipe tables, paragraphs, and inline `code` / `**strong**` / `*em*` /
//! `[links](url)` — and total in the same way: no input is invalid, an unterminated fence runs to
//! the end of the document, and anything unrecognised stays text. Two differences, both forced by
//! the target:
//!
//! * That one builds Leptos views, which cannot be a string on disk. This one builds markup, so
//!   **escaping is the safety property**: every piece of a publisher's text reaches the page
//!   through [`crate::html::escape`], and every link through [`crate::html::safe_href`].
//! * A readme's own headings are shifted down a level (`#` lands as `<h2>`, `##` as `<h3>`),
//!   because the page already has an `<h1>` — the item's name — and a document that brought its
//!   own would leave a page with two.
//!
//! Not read: a table's alignment row is consumed but its colons are ignored, and the columns read
//! left like the prose around them.

use std::fmt::Write as _;

use crate::html::{escape, safe_href};

/// A document as markup, ready to go inside `.prose`.
#[must_use]
pub fn to_html(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = String::with_capacity(source.len() * 2);
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
        } else if trimmed.starts_with("```") {
            i = code_block(&lines, i, &mut out);
        } else if trimmed.starts_with('#') {
            heading(trimmed, &mut out);
            i += 1;
        } else if is_rule(trimmed) {
            out.push_str("<hr>");
            i += 1;
        } else if trimmed.starts_with("> ") || trimmed == ">" {
            i = quote(&lines, i, &mut out);
        } else if bullet(trimmed).is_some() || numbered(trimmed).is_some() {
            i = list(&lines, i, &mut out);
        } else if is_table_header(&lines, i) {
            i = table(&lines, i, &mut out);
        } else {
            i = paragraph(&lines, i, &mut out);
        }
    }
    out
}

/// A fenced block, from the line after the opening fence to the closing one — or to the end of the
/// document, which is what an unterminated fence means and is never an error.
///
/// The info string (```` ```rust ````) is dropped rather than emitted as a class: nothing on these
/// pages highlights, and a class naming a language that is not styled says nothing.
fn code_block(lines: &[&str], start: usize, out: &mut String) -> usize {
    let mut i = start + 1;
    let mut body = String::new();
    while i < lines.len() && !lines[i].trim_start().starts_with("```") {
        body.push_str(lines[i]);
        body.push('\n');
        i += 1;
    }
    out.push_str("<pre><code>");
    out.push_str(&escape(&body));
    out.push_str("</code></pre>");
    // Past the closing fence when there was one; already at the end when there was not.
    i + 1
}

/// `#`-prefixed, shifted down one level and capped at `<h4>` — three visible steps is as deep as
/// the type scale goes (DESIGN.md §4) and deeper outlines were being drawn as bold paragraphs.
fn heading(line: &str, out: &mut String) {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    let text = line[hashes..].trim();
    let level = (hashes + 1).clamp(2, 4);
    let _ = write!(out, "<h{level}>{}</h{level}>", inline(text));
}

/// `---`, `***` or `___` on a line of its own, three or more.
fn is_rule(line: &str) -> bool {
    ['-', '*', '_'].iter().any(|c| {
        let stripped: String = line.chars().filter(|ch| !ch.is_whitespace()).collect();
        stripped.len() >= 3 && stripped.chars().all(|ch| ch == *c)
    })
}

/// The text of a `-`/`*`/`+` item, if the line is one.
fn bullet(line: &str) -> Option<&str> {
    ["- ", "* ", "+ "]
        .iter()
        .find_map(|marker| line.strip_prefix(marker))
}

/// The text of a `1.` item, if the line is one.
fn numbered(line: &str) -> Option<&str> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    line[digits..].strip_prefix(". ")
}

/// A run of list items. The first item decides whether the run is ordered, so a `1.` after a `-`
/// continues the bulleted list rather than opening a second one inside it.
fn list(lines: &[&str], start: usize, out: &mut String) -> usize {
    let ordered = numbered(lines[start].trim()).is_some();
    let tag = if ordered { "ol" } else { "ul" };
    let _ = write!(out, "<{tag}>");
    let mut i = start;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        let Some(text) = bullet(trimmed).or_else(|| numbered(trimmed)) else {
            break;
        };
        let _ = write!(out, "<li>{}</li>", inline(text.trim()));
        i += 1;
    }
    let _ = write!(out, "</{tag}>");
    i
}

/// A run of `>` lines, as one quote.
fn quote(lines: &[&str], start: usize, out: &mut String) -> usize {
    let mut i = start;
    let mut text = String::new();
    while i < lines.len() {
        let trimmed = lines[i].trim();
        let Some(rest) = trimmed.strip_prefix('>') else {
            break;
        };
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(rest.trim());
        i += 1;
    }
    let _ = write!(out, "<blockquote><p>{}</p></blockquote>", inline(text.trim()));
    i
}

/// Whether this line opens a pipe table: a row of cells with a `---|---` delimiter under it.
fn is_table_header(lines: &[&str], i: usize) -> bool {
    let Some(next) = lines.get(i + 1) else {
        return false;
    };
    lines[i].contains('|')
        && next.contains('-')
        && next
            .trim()
            .chars()
            .all(|c| matches!(c, '-' | '|' | ':' | ' '))
        && next.contains('|')
}

/// A pipe table: the header row, the delimiter row (consumed, see the module header), then every
/// row until a line with no pipe in it.
fn table(lines: &[&str], start: usize, out: &mut String) -> usize {
    out.push_str("<table><thead><tr>");
    for cell in cells(lines[start]) {
        let _ = write!(out, "<th>{}</th>", inline(cell));
    }
    out.push_str("</tr></thead><tbody>");
    let mut i = start + 2;
    while i < lines.len() && lines[i].contains('|') {
        out.push_str("<tr>");
        for cell in cells(lines[i]) {
            let _ = write!(out, "<td>{}</td>", inline(cell));
        }
        out.push_str("</tr>");
        i += 1;
    }
    out.push_str("</tbody></table>");
    i
}

/// One row's cells, with the leading and trailing pipes dropped.
fn cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(str::trim)
        .collect()
}

/// Consecutive non-blank lines that are nothing else, joined the way markdown joins them — with a
/// space, not a line break.
fn paragraph(lines: &[&str], start: usize, out: &mut String) -> usize {
    let mut i = start;
    let mut text = String::new();
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("```")
            || trimmed.starts_with("> ")
            || is_rule(trimmed)
            || bullet(trimmed).is_some()
            || numbered(trimmed).is_some()
        {
            break;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(trimmed);
        i += 1;
    }
    let _ = write!(out, "<p>{}</p>", inline(&text));
    i
}

/// Inline markup, walked over the **raw** text.
///
/// Raw rather than pre-escaped because a link's URL has to reach [`safe_href`] as it was written:
/// escaping first would hand it `&amp;` where the publisher wrote `&`, and the href would carry
/// the entity into the query string. So every text run is escaped as it is emitted instead, which
/// is the same guarantee one step later.
fn inline(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut plain = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &text[i..];
        if let Some(end) = code_span(rest) {
            out.push_str(&escape(&plain));
            plain.clear();
            let _ = write!(out, "<code>{}</code>", escape(&rest[1..end]));
            i += end + 1;
        } else if let Some((inner, len)) = wrapped(rest, "**") {
            out.push_str(&escape(&plain));
            plain.clear();
            let _ = write!(out, "<strong>{}</strong>", inline(inner));
            i += len;
        } else if let Some((inner, len)) = wrapped(rest, "*") {
            out.push_str(&escape(&plain));
            plain.clear();
            let _ = write!(out, "<em>{}</em>", inline(inner));
            i += len;
        } else if let Some((label, url, len)) = markdown_link(rest) {
            out.push_str(&escape(&plain));
            plain.clear();
            out.push_str(&anchor(label, url));
            i += len;
        } else {
            let ch = rest.chars().next().unwrap_or_default();
            plain.push(ch);
            i += ch.len_utf8();
        }
    }
    out.push_str(&escape(&plain));
    out
}

/// The index of the backtick closing a code span that starts at `rest[0]`, if there is one.
fn code_span(rest: &str) -> Option<usize> {
    rest.strip_prefix('`')?;
    rest[1..].find('`').map(|end| end + 1)
}

/// The text between a pair of `marker`s at the head of `rest`, and how far past the closing one
/// to continue. An unmatched opener is not emphasis — it is an asterisk somebody typed.
fn wrapped<'a>(rest: &'a str, marker: &str) -> Option<(&'a str, usize)> {
    let after = rest.strip_prefix(marker)?;
    let end = after.find(marker)?;
    if end == 0 {
        return None;
    }
    Some((&after[..end], marker.len() * 2 + end))
}

/// `[label](url)` at the head of `rest`: the label, the URL, and the whole match's length.
fn markdown_link(rest: &str) -> Option<(&str, &str, usize)> {
    let after = rest.strip_prefix('[')?;
    let close = after.find(']')?;
    let tail = after[close + 1..].strip_prefix('(')?;
    let end = tail.find(')')?;
    Some((&after[..close], &tail[..end], close + end + 4))
}

/// A link whose target survived [`safe_href`], or its label as plain text when it did not.
fn anchor(label: &str, url: &str) -> String {
    match safe_href(url) {
        Some(href) => format!("<a href=\"{href}\">{}</a>", inline(label)),
        None => inline(label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_readmes_headings_start_at_h2_so_the_page_keeps_one_h1() {
        assert_eq!(to_html("# Title"), "<h2>Title</h2>");
        assert_eq!(to_html("## What it is"), "<h3>What it is</h3>");
        assert_eq!(to_html("##### deep"), "<h4>deep</h4>");
    }

    #[test]
    fn markup_in_a_publishers_text_is_text() {
        let html = to_html("A <script>alert(1)</script> and a [click](javascript:alert(1)).");
        assert!(!html.contains("<script"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(!html.contains("javascript:"), "{html}");
        assert!(html.contains("click"), "the label survives as text: {html}");
    }

    #[test]
    fn an_ampersand_in_a_link_reaches_the_href_as_the_publisher_wrote_it() {
        let html = to_html("[docs](https://example.com/a?x=1&y=2)");
        assert!(html.contains("href=\"https://example.com/a?x=1&amp;y=2\""), "{html}");
    }

    #[test]
    fn the_blocks_render() {
        assert_eq!(to_html("- one\n- two"), "<ul><li>one</li><li>two</li></ul>");
        assert_eq!(to_html("1. one\n2. two"), "<ol><li>one</li><li>two</li></ol>");
        assert_eq!(to_html("> quoted"), "<blockquote><p>quoted</p></blockquote>");
        assert_eq!(to_html("---"), "<hr>");
        assert_eq!(to_html("```\ncode\n```"), "<pre><code>code\n</code></pre>");
        assert_eq!(
            to_html("| a | b |\n| --- | --- |\n| 1 | 2 |"),
            "<table><thead><tr><th>a</th><th>b</th></tr></thead>\
             <tbody><tr><td>1</td><td>2</td></tr></tbody></table>"
        );
    }

    #[test]
    fn inline_markup_nests_and_an_unmatched_marker_is_a_character() {
        assert_eq!(inline("**bold `code`**"), "<strong>bold <code>code</code></strong>");
        assert_eq!(inline("a * b"), "a * b");
        assert_eq!(inline("`x`"), "<code>x</code>");
    }

    /// An unterminated fence is a document somebody is still writing, not a parse error — the
    /// property `adi_ui::Markdown` has and the reason neither of them returns a `Result`.
    #[test]
    fn an_unterminated_fence_runs_to_the_end() {
        assert_eq!(to_html("```\nstill going"), "<pre><code>still going\n</code></pre>");
    }

    #[test]
    fn a_paragraph_joins_its_lines_with_a_space() {
        assert_eq!(to_html("one\ntwo\n\nthree"), "<p>one two</p><p>three</p>");
    }
}
