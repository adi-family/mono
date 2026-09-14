//! What one entry is, said the same way on both pages.
//!
//! The shelf and the item's own page draw the same six facts — the mark, the name, the line, what
//! it contains, where it comes from, how it is addressed — and the first version of this crate
//! wrote each of them twice. These are the answers; the two pages only decide where to put them.
//!
//! The wording follows `crates/adi-webapp/src/pages/marketplace.rs` deliberately. A person who
//! reads "agent · 2 tools · dashboard" here and then installs the bundle sees the same phrase in
//! the panel, and two spellings of one fact would read as two facts.

use adi_marketplace::{BundleEntry, Kind};

use crate::html::escape;
use crate::icons::Icon;

/// The eight kinds in the order every listing sorts them — `Kind::ALL`'s own order, so the eye
/// learns it once and never re-learns it from item to item.
pub const KINDS: [Kind; 8] = Kind::ALL;

/// What one element of this kind is called, in the singular.
#[must_use]
pub fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Agent => "agent",
        Kind::Tool => "tool",
        Kind::Dashboard => "dashboard",
        Kind::Llm => "LLM backend",
        Kind::Embedding => "embedding backend",
        Kind::Service => "hive service",
        Kind::Trigger => "trigger",
        Kind::Project => "project",
    }
}

/// The same word for more than one of them.
#[must_use]
pub fn kind_plural(kind: Kind) -> String {
    match kind {
        Kind::Llm => "LLM backends".to_string(),
        Kind::Embedding => "embedding backends".to_string(),
        Kind::Service => "hive services".to_string(),
        other => format!("{}s", kind_label(other)),
    }
}

/// The icon for a kind — the control panel's `kind_icon`, element for element.
#[must_use]
pub fn kind_icon(kind: Kind) -> Icon {
    match kind {
        Kind::Agent => Icon::Bot,
        Kind::Tool => Icon::Wrench,
        Kind::Dashboard => Icon::LayoutDashboard,
        Kind::Llm => Icon::Brain,
        Kind::Embedding => Icon::ScanLine,
        Kind::Service => Icon::Server,
        Kind::Trigger => Icon::Zap,
        Kind::Project => Icon::Folder,
    }
}

/// What an entry carries, as a shelf label says it: one phrase per kind it has an element of, in
/// [`KINDS`] order — "agent · 2 tools · dashboard".
///
/// Not "4 elements": the count is the one thing about a bundle nobody is shopping for. Empty for
/// an entry that previews nothing, which is legal and ordinary — the manifest's `elements` is
/// advisory, and what installs is read off the pinned tree.
#[must_use]
pub fn contents_note(entry: &BundleEntry) -> String {
    let elements = entry.elements();
    KINDS
        .iter()
        .filter_map(|kind| {
            let n = elements.iter().filter(|(k, _)| k == kind).count();
            match n {
                0 => None,
                1 => Some(kind_label(*kind).to_string()),
                n => Some(format!("{n} {}", kind_plural(*kind))),
            }
        })
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

/// How an entry is addressed on a machine: `<marketplace>/<slug>`, the argument that goes after
/// `marketplace install`.
#[must_use]
pub fn address(source: &str, entry: &BundleEntry) -> String {
    format!("{source}/{}", entry.slug)
}

/// Where this item's page lives under the store's own root: its slug, and nothing else.
///
/// **Flat on purpose.** The marketplace an item came from is a fact on its page, not a URL
/// segment: a store published at `withadi.dev/store` with a marketplace named `store` would
/// otherwise put every item at `/store/store/<slug>`, and — worse — adding a second marketplace
/// later would move every URL that already existed, which on the one surface built to be found by
/// search is the expensive kind of tidy-up. Two marketplaces publishing one slug is refused at
/// build time instead ([`crate::check`]).
#[must_use]
pub fn page_path(entry: &BundleEntry) -> String {
    entry.slug.clone()
}

/// The command that installs this entry on a machine that already has adi.
#[must_use]
pub fn install_command(source: &str, entry: &BundleEntry) -> String {
    format!("adi-mono marketplace install {}", address(source, entry))
}

/// A repository as a listing says it: the host and path, with the scheme and the `.git` dropped.
///
/// A `file://` repository — a bundle being developed against a local checkout — keeps only its
/// last two segments, because the sixty characters of somebody's home directory in front of them
/// say nothing about whose code it is. The item's own page prints the whole string.
#[must_use]
pub fn repo_short(repo: &str) -> String {
    if let Some(rest) = repo.strip_prefix("https://") {
        return rest.trim_end_matches(".git").to_string();
    }
    let Some(path) = repo.strip_prefix("file://") else {
        return repo.to_string();
    };
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let mut tail: Vec<&str> = path.rsplit('/').take(2).collect();
    tail.reverse();
    format!("\u{2026}/{}", tail.join("/"))
}

/// The pin, at the length a person compares by eye. The whole of it is on the item's page.
#[must_use]
pub fn short_commit(commit: &str) -> String {
    commit.chars().take(7).collect()
}

/// The mark an entry publishes, or the tile it gets when it publishes none.
///
/// **This is the one element on these pages that fetches from a host nobody reading chose** — the
/// manifest's publisher chose it. The store's own validation is what stands between a reader and
/// that: an icon is `https://` or a `data:` URI or it is not drawn at all. `loading="lazy"` keeps
/// a shelf of thirty entries from opening thirty connections before the first screen is painted.
#[must_use]
pub fn icon_html(entry: &BundleEntry, class: &str) -> String {
    match entry.icon() {
        Some(src) => format!(
            "<img class=\"icon {class}\" src=\"{}\" alt=\"\" loading=\"lazy\" decoding=\"async\">",
            escape(src)
        ),
        None => format!(
            "<div class=\"icon icon--blank {class}\">{}</div>",
            Icon::Package.svg("")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_marketplace::Element;

    fn entry(kinds: &[&str]) -> BundleEntry {
        BundleEntry {
            slug: "crm-suite".to_string(),
            name: "CRM suite".to_string(),
            description: None,
            icon: None,
            keywords: Vec::new(),
            readme: None,
            gallery: Vec::new(),
            version: None,
            repo: "https://github.com/adi-family/crm-suite.git".to_string(),
            commit: "9f2c1d4e5a6b7c8d9e0f1a2b3c4d5e6f70819a2b".to_string(),
            branch: None,
            elements: kinds
                .iter()
                .enumerate()
                .map(|(i, kind)| Element {
                    kind: (*kind).to_string(),
                    name: format!("e{i}"),
                    description: None,
                })
                .collect(),
        }
    }

    #[test]
    fn the_contents_note_counts_by_kind_in_one_fixed_order() {
        assert_eq!(
            contents_note(&entry(&["dashboard", "tool", "agent", "tool"])),
            "agent \u{b7} 2 tools \u{b7} dashboard"
        );
        assert_eq!(contents_note(&entry(&["llm", "llm"])), "2 LLM backends");
        assert_eq!(contents_note(&entry(&[])), "", "previewing nothing is legal");
    }

    /// A kind this build has never heard of is left out of the preview rather than failing it —
    /// the tolerance `BundleEntry::elements` is built on, checked here because the note is where
    /// it would otherwise show up as a stray word.
    #[test]
    fn a_kind_from_a_newer_manifest_is_left_out() {
        assert_eq!(contents_note(&entry(&["agent", "hologram"])), "agent");
    }

    #[test]
    fn a_repository_reads_as_host_and_path() {
        assert_eq!(
            repo_short("https://github.com/adi-family/crm.git"),
            "github.com/adi-family/crm"
        );
        assert_eq!(repo_short("file:///Users/x/y/repos/crm"), "\u{2026}/repos/crm");
    }

    #[test]
    fn an_entry_with_no_icon_draws_a_tile_rather_than_a_hole() {
        let html = icon_html(&entry(&[]), "icon--tile");
        assert!(html.contains("icon--blank"), "{html}");
        assert!(html.contains("<svg"), "{html}");
    }

    #[test]
    fn the_address_is_the_install_argument_and_the_url() {
        let entry = entry(&[]);
        assert_eq!(address("adi", &entry), "adi/crm-suite");
        assert_eq!(
            install_command("adi", &entry),
            "adi-mono marketplace install adi/crm-suite"
        );
    }
}
