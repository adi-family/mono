//! Escaping, and the two or three shapes every page here builds text into.
//!
//! Everything on these pages except the words "adi" comes out of **somebody else's manifest** —
//! a publisher's name, description, keywords, readme and URLs. The site is generated once and
//! served as files, so nothing at serve time will sanitize anything: [`escape`] is the whole
//! defence, and the rule is that every interpolated value passes through it or through a function
//! that already has.

/// One text value, safe to drop into element content **or** into a double-quoted attribute.
///
/// Single quotes are escaped along with the rest so the same function serves both, rather than
/// leaving a second, subtly weaker escape for attributes that somebody reaches for by mistake.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// A URL this page is willing to make live, or `None`.
///
/// The manifest's own validation already refuses an icon or a gallery item that is not `https://`
/// or `data:image` / `data:video` — but a *link* (a repository, a link inside a readme) is not
/// covered by it, and `javascript:` in an `href` is the one string on these pages that would
/// become a capability. So links are held to a list of schemes rather than to a list of bad ones:
/// anything not named here is rendered as text, which is what a reader can still copy.
#[must_use]
pub fn safe_href(url: &str) -> Option<String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let ok = ["https://", "http://", "mailto:", "/", "./", "../", "#"]
        .iter()
        .any(|prefix| lower.starts_with(prefix));
    // A bare relative path ("guide.html") has no scheme to abuse. A colon *before* the first
    // slash means the URL is claiming one, and a scheme not named above does not get to have it.
    let relative = match (lower.find(':'), lower.find('/')) {
        (None, _) => true,
        (Some(colon), Some(slash)) => slash < colon,
        (Some(_), None) => false,
    };
    (ok || relative).then(|| escape(url))
}

/// `<a href="…">` for a URL that may not be one — a link when it is, plain text when it is not.
#[must_use]
pub fn link(url: &str, text: &str, class: &str) -> String {
    match safe_href(url) {
        Some(href) => format!("<a class=\"{class}\" href=\"{href}\">{}</a>", escape(text)),
        None => escape(text),
    }
}

/// The `<script type="application/ld+json">` block carrying one piece of structured data.
///
/// `</` is broken up inside the JSON: a string in the document (a description ending in
/// `</script>`, a URL with a `//`) would otherwise close the element early, and the rest of the
/// page would be parsed as markup. JSON's own `\/` escape is legal and means the same thing, so
/// nothing downstream sees a difference.
#[must_use]
pub fn json_ld(value: &serde_json::Value) -> String {
    let json = value.to_string().replace("</", "<\\/");
    format!("<script type=\"application/ld+json\">{json}</script>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dangerous_character_leaves_as_an_entity() {
        assert_eq!(
            escape(r#"<img src=x onerror="alert('x')">&"#),
            "&lt;img src=x onerror=&quot;alert(&#39;x&#39;)&quot;&gt;&amp;"
        );
    }

    #[test]
    fn a_scheme_that_is_not_on_the_list_is_text_rather_than_a_link() {
        assert_eq!(safe_href("https://example.com/a"), Some("https://example.com/a".to_string()));
        assert_eq!(safe_href("./b.html"), Some("./b.html".to_string()));
        assert_eq!(safe_href("guide.html"), Some("guide.html".to_string()));
        assert_eq!(safe_href("a/b?c=1"), Some("a/b?c=1".to_string()));
        assert_eq!(safe_href("javascript:alert(1)"), None);
        assert_eq!(safe_href("JavaScript:alert(1)"), None, "the scheme is case-insensitive");
        assert_eq!(safe_href("data:text/html,<script>"), None);
        assert_eq!(
            link("javascript:alert(1)", "click", "x"),
            "click",
            "the label stays, the target does not"
        );
    }

    #[test]
    fn structured_data_cannot_close_its_own_script_element() {
        let value = serde_json::json!({ "name": "</script><img src=x>" });
        assert!(!json_ld(&value).contains("</script><img"));
        assert!(json_ld(&value).ends_with("</script>"));
    }
}
