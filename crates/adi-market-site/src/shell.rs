//! The document every page here is drawn into: the head a crawler reads, the bar, and the foot.
//!
//! **The head is half the point of this site.** The control panel already lists these bundles, and
//! it lists them well — but it is a wasm application behind somebody's own machine, so nothing
//! outside can read it and nobody can link to one item. These pages exist so an item has a URL
//! that answers with its name, its description and its structured data in the first response,
//! before any script runs.
//!
//! Nothing on these pages *needs* a script: every word, every command and both answers to "have
//! you got adi" are in the markup, and [`crate::assets`] ships one stylesheet. What script there
//! is ([`crate::get`]) only decides which of the two answers is in front, which is why the
//! three lines of it that must beat the first paint are inlined here rather than fetched.
//!
//! Every internal link is **relative** (`../site.css`), so the same output directory serves
//! correctly at a domain root, under a path prefix on a static host — which is what
//! `withadi.dev/store` is — and from `file://`.

use crate::html::{escape, json_ld};
use crate::icons::Icon;
use crate::{Site, links};

/// The mark at bar size, monochrome — see the file's own comment for what pins it.
const MARK: &str = include_str!("../assets/mark.svg");

/// The word after the wordmark in the bar. Not the site's configured name: the bar says where
/// you are in two words, and "ADI Store — starter apps" is a `<title>`, not a location.
const HERE: &str = "store";

/// The only thing on this site that has to run before the first paint: which of the two answers to
/// "have you got adi" this reader has already given, read back onto `<html>` so the stylesheet can
/// hide the other one without it being drawn first and taken away.
///
/// Inline rather than in `site.js`, because a deferred file runs *after* paint and the swap would
/// be a visible flicker on every page. `class="js"` is the other half: the controls that ask the
/// question are hidden until something can answer it, so a reader with no script never sees a
/// button that does nothing.
const STATE: &str = "<script>try{document.documentElement.className='js';\
                     document.documentElement.dataset.adi=\
                     localStorage.getItem('adi.installed')||''}catch(e){}</script>";

/// What a crawler and a link preview are given for one page.
#[derive(Debug, Clone)]
pub struct Head {
    /// The `<title>`, whole — including whatever suffix the page wants.
    pub title: String,
    /// One sentence, for `<meta name="description">` and the link preview under it.
    pub description: String,
    /// This page's absolute address, for `<link rel="canonical">` and `og:url`. Without one the
    /// page still renders; a site built for publication always has one.
    pub canonical: String,
    /// The picture a link preview shows — an entry's own icon, when it publishes one.
    pub image: Option<String>,
    /// The structured data blocks, each already a JSON-LD object.
    pub data: Vec<serde_json::Value>,
}

/// One page, whole.
///
/// `depth` is how far this page sits below the store's root (0 for the shelf, 1 for an item at
/// `<slug>/`), and everything relative is built from it.
#[must_use]
pub fn document(site: &Site, head: &Head, depth: usize, body: &str) -> String {
    let up = up(depth);
    let data = head
        .data
        .iter()
        .map(json_ld)
        .collect::<Vec<_>>()
        .join("\n");
    let canonical = if head.canonical.is_empty() {
        String::new()
    } else {
        format!(
            "<link rel=\"canonical\" href=\"{0}\">\n<meta property=\"og:url\" content=\"{0}\">\n",
            escape(&head.canonical)
        )
    };
    let image = head.image.as_deref().map_or_else(String::new, |src| {
        format!(
            "<meta property=\"og:image\" content=\"{0}\">\n\
             <meta name=\"twitter:image\" content=\"{0}\">\n",
            escape(src)
        )
    });
    // `summary_large_image` only when there is an image to be large: the small card is the honest
    // one for an entry that publishes no icon, and X renders a broken card rather than falling
    // back when the picture is missing.
    let card = if head.image.is_some() {
        "summary_large_image"
    } else {
        "summary"
    };
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"color-scheme\" content=\"dark\">\n\
         {STATE}\n\
         <title>{title}</title>\n\
         <meta name=\"description\" content=\"{description}\">\n\
         {canonical}\
         <meta property=\"og:type\" content=\"website\">\n\
         <meta property=\"og:site_name\" content=\"{site_name}\">\n\
         <meta property=\"og:title\" content=\"{title}\">\n\
         <meta property=\"og:description\" content=\"{description}\">\n\
         {image}\
         <meta name=\"twitter:card\" content=\"{card}\">\n\
         <link rel=\"icon\" href=\"{up}favicon.svg\" type=\"image/svg+xml\">\n\
         <link rel=\"icon\" href=\"{up}favicon.png\" sizes=\"any\">\n\
         <link rel=\"stylesheet\" href=\"{up}site.css\">\n\
         <script src=\"{up}site.js\" defer></script>\n\
         {data}\n</head>\n<body>\n{bar}\n<main>\n{body}\n</main>\n{foot}\n</body>\n</html>\n",
        title = escape(&head.title),
        description = escape(&head.description),
        site_name = escape(&site.name),
        bar = bar(&up),
        foot = foot(),
    )
}

/// The relative prefix that reaches the site root from `depth` levels down.
fn up(depth: usize) -> String {
    "../".repeat(depth)
}

/// The bar: the mark, where you are, and the one link off this site that a reader who likes what
/// they see actually needs.
fn bar(up: &str) -> String {
    // An empty href is the current *document*, not the directory, so the shelf's own links to
    // itself are written out rather than left blank.
    let root = if up.is_empty() { "./" } else { up };
    format!(
        "<header class=\"bar\"><div class=\"wrap\">\
         <a class=\"brand\" href=\"{root}\">{MARK}adi <span class=\"here\">{HERE}</span></a>\
         <nav>\
         <a href=\"{root}\">All apps</a>\
         <a class=\"wide-only\" href=\"{docs}\">How it works{arrow}</a>\
         <a class=\"if-no-adi\" href=\"{get}\">Get adi</a>\
         <a class=\"only-adi\" href=\"{panel}\">Your panel{arrow}</a>\
         </nav></div></header>",
        docs = links::MARKETPLACE_DOCS,
        // The site's own page, not withadi.dev: the reader is being asked to do something, and
        // the thing to do depends on what they already have. The foot still links the product.
        get = crate::get::href(root, None),
        panel = links::panel_market(),
        arrow = Icon::ArrowUpRight.svg("i--sm"),
    )
}

/// The foot: what this page *is*, which is the question a reader arriving from a search engine has
/// and the panel's own listing never has to answer.
fn foot() -> String {
    format!(
        "<footer class=\"foot\"><div class=\"wrap\">\
         <p>Generated from a marketplace manifest \u{2014} one JSON file its publisher hosts. \
         Nothing is hosted here and there is no account: installing clones a git repository at \
         the commit the manifest pins, onto your own machine.</p>\
         <span class=\"spacer\"></span>\
         <a href=\"{adi}\">withadi.dev</a>\
         <a href=\"{docs}\">The manifest format</a>\
         </div></footer>",
        adi = links::ADI,
        docs = links::MARKETPLACE_DOCS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Site {
        Site {
            name: "ADI Store".to_string(),
            base_url: "https://example.com".to_string(),
        }
    }

    fn head() -> Head {
        Head {
            title: "A page".to_string(),
            description: "What it is.".to_string(),
            canonical: "https://example.com/a/b/".to_string(),
            image: None,
            data: Vec::new(),
        }
    }

    /// The mark is a copy of one drawing, and this is what keeps it one. Every lobe in the file
    /// beside this crate must appear, character for character, in the mark the app icon is
    /// generated from.
    #[test]
    fn the_mark_agrees_with_the_canonical_drawing() {
        let canonical = include_str!("../../adi-webapp/assets/mark.svg");
        let paths: Vec<&str> = MARK
            .match_indices(" d=\"")
            .map(|(at, _)| {
                let rest = &MARK[at + 4..];
                &rest[..rest.find('"').expect("a closing quote")]
            })
            .collect();
        assert_eq!(paths.len(), 6, "three lobes, and the three paths the two cuts are made of");
        for path in paths {
            assert!(canonical.contains(path), "this lobe has drifted: {path}");
        }
    }

    #[test]
    fn a_page_two_levels_down_reaches_the_stylesheet_and_the_root() {
        let html = document(&site(), &head(), 2, "<p>hi</p>");
        assert!(html.contains("href=\"../../site.css\""), "{html}");
        assert!(html.contains("src=\"../../site.js\""), "{html}");
        assert!(html.contains("class=\"brand\" href=\"../../\""), "{html}");
        assert!(html.contains("content=\"https://example.com/a/b/\""), "canonical: {html}");
    }

    /// Two scripts on a page with no structured data: the three inline lines that must beat the
    /// first paint, and the deferred file. Nothing else, and no behaviour written into the markup.
    #[test]
    fn the_only_script_is_the_one_that_swaps_the_two_answers() {
        let html = document(&site(), &head(), 0, "<p>hi</p>");
        assert_eq!(html.matches("<script").count(), 2, "{html}");
        assert!(html.contains("<script src=\"site.js\" defer>"), "{html}");
        assert!(html.contains("localStorage.getItem('adi.installed')"), "{html}");
        assert!(!html.contains("onclick"), "no behaviour in the markup: {html}");
    }

    #[test]
    fn the_shelf_links_to_itself_without_a_prefix() {
        let html = document(&site(), &head(), 0, "");
        assert!(html.contains("href=\"site.css\""), "{html}");
        assert!(html.contains("class=\"brand\" href=\"./\""), "{html}");
    }

    #[test]
    fn a_page_with_no_picture_asks_for_the_small_card() {
        let html = document(&site(), &head(), 0, "");
        assert!(html.contains("content=\"summary\""), "{html}");
        assert!(!html.contains("og:image"), "{html}");
    }

    #[test]
    fn the_title_and_description_are_escaped() {
        let mut head = head();
        head.title = "A \" onload=\"x".to_string();
        let html = document(&site(), &head, 0, "");
        assert!(!html.contains("onload=\"x\">"), "{html}");
        assert!(html.contains("&quot; onload=&quot;x"), "{html}");
    }
}
