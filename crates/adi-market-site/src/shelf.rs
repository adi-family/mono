//! The shelf: everything published, and one sentence saying what any of it is.
//!
//! It answers two readers at once, and the order of the page is which one it answers first. A
//! **stranger off a search engine** does not know what adi is, so the hero says what installing
//! something here does to their machine before a single entry is named. Somebody who **already
//! runs adi** wants the address to paste, so every section carries the one command that adds
//! that marketplace to their own machine.
//!
//! A row is a link and carries nothing to press, which is the panel's own rule for its listing
//! (`crates/adi-webapp/src/pages/marketplace.rs`): the shelf is for browsing, and an item's page
//! is where somebody acts.

use adi_marketplace::BundleEntry;

use crate::entry::{contents_note, icon_html, repo_short, short_commit};
use crate::html::escape;
use crate::shell::{self, Head};
use crate::{Site, Source, links};

/// The headline. One sentence, in the display face, under 32ch — DESIGN.md §5.
const HEADLINE: &str = "Apps you run on your own machine.";

/// The paragraph under it: what an item here *is*, in the terms that make the rest of the page
/// legible. Every clause is load-bearing — a git repository, one commit, your machine, inert on
/// arrival — and it is the same promise the item pages repeat as three lines with icons.
const LEDE: &str = "Agents, tools, dashboards, model backends and services for adi, published as \
                    git repositories pinned to one commit. Installing one clones it onto your own \
                    machine. There is no account, nothing is hosted here, and nothing runs until \
                    you start it.";

/// The site's own page.
#[must_use]
pub fn render(site: &Site, sources: &[Source]) -> String {
    let body = format!(
        "{hero}<div class=\"wrap\">{sections}</div>",
        hero = hero(),
        sections = sources
            .iter()
            .map(section)
            .collect::<String>(),
    );
    shell::document(site, &head(site, sources), 0, &body)
}

/// What a crawler is told this page is, and the list itself as structured data.
fn head(site: &Site, sources: &[Source]) -> Head {
    let items: Vec<serde_json::Value> = sources
        .iter()
        .flat_map(|source| {
            source.manifest.bundles.iter().map(move |entry| {
                serde_json::json!({
                    "@type": "ListItem",
                    "name": entry.name,
                    "url": site.item_url(entry),
                })
            })
        })
        .enumerate()
        .map(|(i, mut item)| {
            item["position"] = serde_json::json!(i + 1);
            item
        })
        .collect();
    Head {
        title: format!(
            "{} \u{2014} agents, tools and dashboards you install on your own machine",
            site.name
        ),
        description: LEDE.to_string(),
        canonical: site.url("/"),
        image: None,
        data: vec![serde_json::json!({
            "@context": "https://schema.org",
            "@type": "CollectionPage",
            "name": site.name,
            "description": LEDE,
            "url": site.url("/"),
            "mainEntity": { "@type": "ItemList", "itemListElement": items },
        })],
    }
}

/// The hero. Its button is the page's one orange (DESIGN.md §4): for a reader who does not have
/// adi, nothing else on the shelf is an action at all — and for one who does, the orange moves to
/// the panel link, because by then that is the only act left on the page (`.btn--promoted`).
///
/// The panel group is `only-adi`, not `if-adi`: the item pages carry both answers side by side
/// because the commands there are worth reading either way, but on the shelf the second group is
/// one more control in the same line of sight and answers nothing a browsing reader asked.
fn hero() -> String {
    format!(
        "<section class=\"hero\"><div class=\"wrap\">\
         <h1>{HEADLINE}</h1>\
         <p class=\"lede\">{LEDE}</p>\
         <div class=\"hero__actions\">\
         <span class=\"hero__group if-no-adi\">\
         <a class=\"btn btn--primary\" href=\"{get}\">Get adi</a>\
         <span class=\"label\">Free while in beta \u{b7} macOS, Linux and Windows</span>\
         {ask_yes}\
         </span>\
         <span class=\"hero__group only-adi\">\
         <a class=\"btn btn--promoted\" href=\"{panel}\">Open your panel</a>\
         {ask_no}\
         </span>\
         </div></div></section>",
        get = crate::get::href("./", None),
        panel = links::panel_market(),
        ask_yes = crate::get::toggle(false),
        ask_no = crate::get::toggle(true),
    )
}

/// One marketplace: what it is called, how to add it, and its entries.
fn section(source: &Source) -> String {
    let name = source
        .manifest
        .name
        .as_deref()
        .map_or_else(|| source.name.clone(), str::to_string);
    let entries = if source.manifest.bundles.is_empty() {
        "<p class=\"empty\">Nothing published in this manifest yet.</p>".to_string()
    } else {
        format!(
            "<div class=\"shelf\">{}</div>",
            source
                .manifest
                .bundles
                .iter()
                .map(tile)
                .collect::<String>()
        )
    };
    format!(
        "<section class=\"source\">\
         <div class=\"source__head\">\
         <p class=\"eyebrow\">Marketplace</p>\
         <div class=\"source__name\"><h2>{name}</h2><span class=\"label\">{count}</span></div>\
         </div>\
         {add}{entries}</section>",
        name = escape(&name),
        count = items(source.manifest.bundles.len()),
        add = add_command(source),
    )
}

/// "5 items" — the one count on this site, and it is a fact about a file rather than about
/// anybody's popularity. Install counts are deliberately nowhere (`docs/marketplace.md`).
fn items(n: usize) -> String {
    match n {
        1 => "1 item".to_string(),
        n => format!("{n} items"),
    }
}

/// The line that puts this marketplace on somebody's own machine, and the label that says what
/// the name above it *is*.
///
/// The label is not filler. A bare heading reading "Local bundles" was taken as a claim about the
/// items — are these local, as opposed to remote ones? — when it is only the name its publisher
/// gave the manifest. Saying "Marketplace" over it, and printing the URL it is fetched from right
/// underneath, answers both questions in the reader's first glance.
///
/// Omitted when the site was built without saying where the manifest is published: a command with
/// a guess in it is worse than no command.
fn add_command(source: &Source) -> String {
    let Some(command) = crate::get::add_command(source) else {
        return String::new();
    };
    format!(
        "<div class=\"if-adi source__add\">\
         <p class=\"label\">Add it to your own adi:</p>\
         <p class=\"cmd\"><span class=\"prompt\">$</span><code>{}</code></p>\
         </div>",
        escape(&command),
    )
}

/// One entry on the shelf. The whole row is the link: every part of it is about the same item, and
/// a store where only the four words of the name are clickable is a store people think is broken.
fn tile(entry: &BundleEntry) -> String {
    let contents = contents_note(entry);
    let meta = if contents.is_empty() {
        format!(
            "{} @ {}",
            repo_short(&entry.repo),
            short_commit(&entry.commit)
        )
    } else {
        contents
    };
    format!(
        "<a class=\"tile\" href=\"{href}/\">{icon}\
         <div class=\"tile__about\">\
         <div class=\"tile__name\">{name}{version}</div>\
         {description}\
         <div class=\"tile__meta\">{meta}</div>\
         </div></a>",
        href = escape(&crate::entry::page_path(entry)),
        icon = icon_html(entry, "icon--tile"),
        name = escape(&entry.name),
        version = entry.version.as_deref().map_or_else(String::new, |v| format!(
            "<span class=\"mono\">{}</span>",
            escape(v.trim())
        )),
        description = entry
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map_or_else(String::new, |d| format!(
                "<div class=\"tile__desc\">{}</div>",
                escape(d)
            )),
        meta = escape(&meta),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{fixture_site, fixture_source};

    #[test]
    fn every_entry_is_a_link_to_its_own_page_and_no_row_is_a_control() {
        let html = render(&fixture_site(), &[fixture_source()]);
        assert!(html.contains("href=\"crm-suite/\""), "{html}");
        assert!(html.contains("href=\"plain/\""), "{html}");
        let shelf = html.split("<div class=\"shelf\">").nth(1).expect("the shelf");
        assert!(!shelf.contains("<button"), "a row presses nothing: {shelf}");
    }

    /// In every state the reader can be in, exactly one element is filled orange: Get adi while
    /// that is the act, the panel link once it is not (DESIGN.md §4, and `.btn--promoted`).
    #[test]
    fn one_orange_in_each_state() {
        let html = render(&fixture_site(), &[fixture_source()]);
        assert_eq!(html.matches("btn--primary").count(), 1, "{html}");
        assert_eq!(html.matches("btn--promoted").count(), 1, "{html}");
    }

    #[test]
    fn the_shelf_says_what_each_entry_contains() {
        let html = render(&fixture_site(), &[fixture_source()]);
        assert!(html.contains("agent \u{b7} tool \u{b7} dashboard"), "{html}");
    }

    /// An entry that previews no elements still needs a second line, so it falls back to where it
    /// comes from — the fact the panel's row carries for every entry.
    #[test]
    fn an_entry_previewing_nothing_says_where_it_comes_from_instead() {
        let html = render(&fixture_site(), &[fixture_source()]);
        assert!(html.contains("github.com/adi-family/plain @ 9f2c1d4"), "{html}");
    }

    #[test]
    fn the_manifests_own_address_is_offered_when_the_site_knows_it() {
        let html = render(&fixture_site(), &[fixture_source()]);
        assert!(
            html.contains("adi-mono marketplace add local https://example.com/m.json"),
            "{html}"
        );
    }

    #[test]
    fn the_list_is_published_as_structured_data() {
        let html = render(&fixture_site(), &[fixture_source()]);
        assert!(html.contains("\"@type\":\"ItemList\""), "{html}");
        assert!(
            html.contains("https://market.example.com/crm-suite/"),
            "absolute URLs in structured data: {html}"
        );
    }
}
