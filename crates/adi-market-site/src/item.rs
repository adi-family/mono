//! One item's page: what it is, what it looks like, what is in it, and what would be cloned.
//!
//! The order is the control panel's, for the reason written there: the head, then the pictures,
//! then the promise the machine can actually keep, then the contents, then the long form, and
//! only last the machine facts. Somebody deciding whether to install this is not reading a commit
//! id, and somebody checking a commit id knows where to scroll.
//!
//! What this page has that the panel's cannot is a **URL** — and the URL is the install address
//! (`<marketplace>/<slug>`), so the thing a reader copies out of the address bar is the thing they
//! paste after `marketplace install`.

// Every list on this page is built by mapping a handful of values into markup and collecting the
// result. `clippy::format_collect` would have each one be a `fold` with a `write!` inside it to
// save the intermediate allocations — on lists that are never more than a dozen items long, and at
// the cost of the one shape that makes these functions readable side by side.
#![allow(clippy::format_collect)]

use adi_marketplace::{BundleEntry, MediaKind};

use crate::entry::{
    address, icon_html, install_command, kind_icon, kind_label, repo_short, short_commit,
};
use crate::html::{escape, link};
use crate::icons::Icon;
use crate::shell::{self, Head};
use crate::{Site, Source, links, markdown};

/// The page.
#[must_use]
pub fn render(site: &Site, source: &Source, entry: &BundleEntry) -> String {
    let body = format!(
        "<div class=\"wrap\">{back}<article class=\"item\">\
         {hero}{assurances}{gallery}{included}{readme}{facts}\
         </article></div>",
        back = back(),
        hero = hero(source, entry),
        assurances = assurances(),
        gallery = gallery(entry),
        included = included(entry),
        readme = readme(entry),
        facts = facts(source, entry),
    );
    shell::document(site, &head(site, source, entry), 2, &body)
}

/// What a crawler, a link preview and a search result are given for this item.
fn head(site: &Site, source: &Source, entry: &BundleEntry) -> Head {
    let url = site.item_url(&source.name, entry);
    let description = description(entry);
    let mut data = serde_json::json!({
        "@context": "https://schema.org",
        "@type": "SoftwareApplication",
        "name": entry.name,
        "description": description,
        "url": url,
        "applicationCategory": "DeveloperApplication",
        "operatingSystem": "macOS, Linux",
        "codeRepository": entry.repo,
        "isAccessibleForFree": true,
        // Priced explicitly at zero rather than left out: a `SoftwareApplication` with no offer is
        // eligible for nothing, and "free" is a true and useful thing to say about all of this.
        "offers": { "@type": "Offer", "price": "0", "priceCurrency": "USD" },
    });
    if let Some(version) = entry.version.as_deref() {
        data["softwareVersion"] = serde_json::json!(version);
    }
    if !entry.keywords().is_empty() {
        data["keywords"] = serde_json::json!(entry.keywords().join(", "));
    }
    if let Some(image) = preview_image(entry) {
        data["image"] = serde_json::json!(image);
    }
    Head {
        title: format!("{} \u{2014} {}", entry.name, site.name),
        description,
        canonical: url.clone(),
        image: preview_image(entry),
        data: vec![data, breadcrumb(site, source, entry)],
    }
}

/// The trail a search result prints under its title: the shelf, then this item.
fn breadcrumb(site: &Site, source: &Source, entry: &BundleEntry) -> serde_json::Value {
    serde_json::json!({
        "@context": "https://schema.org",
        "@type": "BreadcrumbList",
        "itemListElement": [
            { "@type": "ListItem", "position": 1, "name": site.name, "item": site.url("/") },
            { "@type": "ListItem", "position": 2, "name": entry.name,
              "item": site.item_url(&source.name, entry) },
        ],
    })
}

/// The sentence this page is summarized by, everywhere a summary is asked for.
///
/// The publisher's own line when there is one. When there is not, a sentence built from what the
/// entry does say — its name and its contents — rather than a blank, because an empty meta
/// description is the one that gets rewritten by the search engine into whatever it finds first.
fn description(entry: &BundleEntry) -> String {
    if let Some(line) = entry
        .description
        .as_deref()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        return line.to_string();
    }
    let contents = crate::entry::contents_note(entry);
    if contents.is_empty() {
        format!("{} \u{2014} a bundle for adi, installed from a pinned git commit.", entry.name)
    } else {
        format!("{} \u{2014} {contents} for adi, installed from a pinned git commit.", entry.name)
    }
}

/// The picture a link preview should use: the first published screenshot, else the entry's mark.
///
/// `https://` only, and a `data:` URI is skipped rather than inlined — a preview crawler fetches
/// the URL it is given from somewhere else entirely, and a 40 KB base64 string in a meta tag is
/// a picture nobody ever sees.
fn preview_image(entry: &BundleEntry) -> Option<String> {
    let shot = entry
        .gallery()
        .into_iter()
        .find(|media| media.kind() == MediaKind::Image)
        .map(|media| media.url().to_string());
    shot.or_else(|| entry.icon().map(str::to_string))
        .filter(|url| url.starts_with("https://"))
}

/// Back to the shelf — two levels up, since an item lives at `<marketplace>/<slug>/`.
fn back() -> String {
    format!(
        "<a class=\"back\" href=\"../../\">{}All apps</a>",
        Icon::ArrowLeft.svg("i--sm")
    )
}

/// The head of the page: the mark, the name, the publisher's line, the tags — and the one act.
fn hero(source: &Source, entry: &BundleEntry) -> String {
    let tags = entry.keywords();
    format!(
        "<header class=\"item__hero\">{icon}\
         <div class=\"item__about\">\
         <div class=\"item__title\"><h1>{name}</h1>{version}</div>\
         {description}{tags}\
         </div>\
         {act}</header>",
        icon = icon_html(entry, "icon--hero"),
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
                "<p class=\"lede\">{}</p>",
                escape(d)
            )),
        tags = if tags.is_empty() {
            String::new()
        } else {
            format!(
                "<div class=\"tags\">{}</div>",
                tags.iter()
                    .map(|word| format!("<span class=\"tag\">{}</span>", escape(word)))
                    .collect::<String>()
            )
        },
        act = act(source, entry),
    )
}

/// The one act, in both of the states a reader can be in.
///
/// A page cannot look at somebody's disk (`crate::get`), so both answers are here and the stranger
/// gets the one in front: **Get adi**. Under it, and still readable with no script at all, are the
/// two lines that install this bundle on a machine that already has adi — in the order they have
/// to be run.
///
/// **The `add` line is not decoration.** `marketplace install local/crm-suite` names a marketplace,
/// and a machine that has never added it has never heard of it: the install fails on the address.
/// The first version of this page printed only the second line, which was a command that works on
/// exactly one machine in the world — the one that already followed this manifest.
fn act(source: &Source, entry: &BundleEntry) -> String {
    let address = address(&source.name, entry);
    let add = crate::get::add_command(source)
        .map(|command| {
            crate::get::step(
                &format!("Add the {} marketplace \u{2014} once, on this machine", source.title()),
                Some(&command),
                "",
            )
        })
        .unwrap_or_default();
    format!(
        "<div class=\"item__act\">\
         <div class=\"if-no-adi\">\
         <a class=\"btn btn--primary\" href=\"{get}\">Get adi</a>\
         <p class=\"label\">Free, runs on your own machine, and nothing here starts by itself.</p>\
         {ask_yes}\
         </div>\
         <div class=\"if-adi\">\
         <p class=\"label\">Already running adi? Two lines:</p>\
         <ol class=\"steps\">{add}{install}</ol>\
         <a class=\"btn btn--promoted\" href=\"{panel}\">Open it in your panel</a>\
         {ask_no}\
         </div></div>",
        get = crate::get::href("../../", Some(&address)),
        ask_yes = crate::get::toggle(false),
        install = crate::get::step(
            "Install it",
            Some(&install_command(&source.name, entry)),
            "",
        ),
        panel = escape(&links::panel_item(&source.name, &entry.slug)),
        ask_no = crate::get::toggle(true),
    )
}

/// The three things installing does, in the words the panel uses on the same block. They are
/// promises about *this machine*, and a public page is exactly where somebody is deciding whether
/// to believe them.
fn assurances() -> String {
    let lines = [
        (Icon::Laptop, "Cloned to your machine \u{2014} no account, nothing phoned home"),
        (Icon::Power, "Installed is not started: you decide what runs"),
        (
            Icon::GitCommitHorizontal,
            "Pinned to one commit \u{2014} what you read is what installs",
        ),
    ];
    format!(
        "<ul class=\"assurances\">{}</ul>",
        lines
            .into_iter()
            .map(|(icon, text)| format!("<li>{}<span>{text}</span></li>", icon.svg("i--lg")))
            .collect::<String>()
    )
}

/// The pictures and clips, in published order, the first one across the page.
///
/// Nothing plays by itself: a clip is a `<video controls>` with `preload="metadata"`, so a page of
/// them costs a listing of bytes rather than the clips themselves, and a reader is never
/// ambushed by sound.
fn gallery(entry: &BundleEntry) -> String {
    let media = entry.gallery();
    if media.is_empty() {
        return String::new();
    }
    let shots = media
        .into_iter()
        .map(|item| {
            let frame = match item.kind() {
                MediaKind::Image => format!(
                    "<img src=\"{}\" alt=\"{}\" loading=\"lazy\" decoding=\"async\">",
                    escape(item.url()),
                    escape(item.caption().unwrap_or_default()),
                ),
                MediaKind::Video => format!(
                    "<video controls preload=\"metadata\"{poster} src=\"{}\"></video>",
                    escape(item.url()),
                    poster = item.poster().map_or_else(String::new, |poster| format!(
                        " poster=\"{}\"",
                        escape(poster)
                    )),
                ),
            };
            let caption = item.caption().map_or_else(String::new, |caption| {
                format!("<figcaption>{}</figcaption>", escape(caption))
            });
            format!("<figure class=\"shot\">{frame}{caption}</figure>")
        })
        .collect::<String>();
    format!("<section class=\"section\"><div class=\"gallery\">{shots}</div></section>")
}

/// **What's included**: every element the manifest previews, flat, in kind order, with the kind on
/// the row.
///
/// Flat rather than eight headings over eight single rows — a bundle of one of everything reads as
/// an outline that way, rather than as a list of things you can have. The kind is the repeated
/// word per row, so it is dimmed and it is a column (§8, "repeated column values dimmed").
fn included(entry: &BundleEntry) -> String {
    let mut elements = entry.elements();
    if elements.is_empty() {
        return String::new();
    }
    elements.sort_by_key(|(kind, _)| crate::entry::KINDS.iter().position(|k| k == kind));
    let rows = elements
        .into_iter()
        .map(|(kind, element)| {
            format!(
                "<div class=\"row\">{icon}<span class=\"row__kind\">{kind}</span>\
                 <span class=\"row__name\">{name}</span>\
                 <span class=\"row__desc\">{description}</span></div>",
                icon = kind_icon(kind).svg(""),
                kind = kind_label(kind),
                name = escape(element.name()),
                description = escape(element.description().unwrap_or_default()),
            )
        })
        .collect::<String>();
    format!(
        "<section class=\"section\"><h2>What's included</h2>\
         <p class=\"section__note\">As published. What actually installs is read off the pinned \
         commit's own tree.</p>\
         <div class=\"rows\">{rows}</div></section>"
    )
}

/// The long form, as its publisher wrote it.
fn readme(entry: &BundleEntry) -> String {
    let Some(source) = entry.readme() else {
        return String::new();
    };
    format!(
        "<section class=\"section\"><h2>About</h2><div class=\"prose\">{}</div></section>",
        markdown::to_html(source)
    )
}

/// Where it comes from: the repository, the pin, the branch, and the address to install it by.
///
/// Last on the page and never first. The repository is a link because a reader who has got this
/// far may well want to read the code before running any of it — which is the whole argument for
/// a marketplace that ships repositories rather than artifacts.
fn facts(source: &Source, entry: &BundleEntry) -> String {
    let mut rows = vec![
        (
            "Repository",
            link(&entry.repo, &repo_short(&entry.repo), "mono"),
        ),
        (
            "Commit",
            format!("<span class=\"mono\">{}</span>", escape(&entry.pin())),
        ),
    ];
    if let Some(branch) = entry.branch() {
        rows.push((
            "Branch",
            format!("<span class=\"mono\">{}</span>", escape(branch)),
        ));
    }
    rows.push((
        "Address",
        format!(
            "<span class=\"mono\">{}</span>",
            escape(&address(&source.name, entry))
        ),
    ));
    rows.push(("Marketplace", escape(source.title())));
    if let Some(url) = source.url.as_deref() {
        rows.push(("Manifest", link(url, url, "mono")));
    }
    let list = rows
        .into_iter()
        .map(|(key, value)| format!("<div class=\"fact\"><dt>{key}</dt><dd>{value}</dd></div>"))
        .collect::<String>();
    format!(
        "<section class=\"section\"><h2>Where it comes from</h2>\
         <p class=\"section__note\">Installing clones this repository at this commit \u{2014} \
         {short}. A publisher who pushes something else afterwards changes nothing about what \
         you already read.</p>\
         <dl class=\"facts\">{list}</dl></section>",
        short = short_commit(&entry.commit),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{fixture_site, fixture_source};

    fn page(slug: &str) -> String {
        let source = fixture_source();
        let entry = source
            .manifest
            .bundle(slug)
            .expect("the fixture carries it")
            .clone();
        render(&fixture_site(), &source, &entry)
    }

    #[test]
    fn the_page_carries_one_h1_and_one_orange() {
        let html = page("crm-suite");
        assert_eq!(html.matches("<h1>").count(), 1, "{html}");
        assert_eq!(html.matches("btn--primary").count(), 1, "DESIGN.md \u{a7}4");
    }

    /// The two lines, in the order they have to be run. `install local/crm-suite` names a
    /// marketplace, and a machine that has not added it fails on the address — so a page that
    /// printed only the second line printed a command that works nowhere.
    #[test]
    fn the_marketplace_is_added_before_anything_is_installed_from_it() {
        let html = page("crm-suite");
        let add = html.find("adi-mono marketplace add local").expect("the add line");
        let install = html
            .find("adi-mono marketplace install local/crm-suite")
            .expect("the install line");
        assert!(add < install, "{html}");
    }

    /// Both answers to "have you got adi" are in the markup — the script only takes one away — so
    /// a reader with no script gets the button *and* the commands.
    #[test]
    fn the_page_carries_both_answers_and_a_way_into_the_panel() {
        let html = page("crm-suite");
        assert!(html.contains("class=\"if-no-adi\""), "{html}");
        assert!(html.contains("class=\"if-adi\""), "{html}");
        assert!(html.contains("href=\"http://app.adi/marketplace/local/crm-suite\""), "{html}");
        assert!(html.contains("data-adi-set=\"yes\""), "the question is asked: {html}");
    }

    #[test]
    fn the_install_address_is_the_url_and_the_command() {
        let html = page("crm-suite");
        assert!(html.contains("adi-mono marketplace install local/crm-suite"), "{html}");
        assert!(
            html.contains("<link rel=\"canonical\" href=\"https://market.example.com/local/crm-suite/\">"),
            "{html}"
        );
    }

    #[test]
    fn what_is_included_is_listed_in_kind_order_with_the_kind_dimmed() {
        let html = page("crm-suite");
        let agent = html.find("sales-bot").expect("the agent");
        let dashboard = html.find("crm-board").expect("the dashboard");
        assert!(agent < dashboard, "agents come before dashboards");
        assert!(html.contains("<span class=\"row__kind\">LLM backend</span>"), "{html}");
    }

    #[test]
    fn a_clip_waits_to_be_asked() {
        let html = page("crm-suite");
        assert!(html.contains("<video controls preload=\"metadata\""), "{html}");
        assert!(!html.contains("autoplay"), "{html}");
    }

    #[test]
    fn the_long_form_is_rendered_and_its_markup_is_not() {
        let html = page("crm-suite");
        assert!(html.contains("<h3>What it is</h3>"), "{html}");
        assert!(!html.contains("<script>alert"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "the attempt survives as text: {html}");
    }

    /// An entry that publishes no description still owes a search result a sentence, and the one
    /// it gets is built from what it does publish.
    #[test]
    fn a_missing_description_becomes_one_rather_than_a_blank() {
        let html = page("plain");
        assert!(
            html.contains("<meta name=\"description\" content=\"Plain bundle \u{2014} a bundle for adi"),
            "{html}"
        );
    }

    #[test]
    fn the_structured_data_names_the_repository_and_the_price() {
        let html = page("crm-suite");
        assert!(html.contains("\"@type\":\"SoftwareApplication\""), "{html}");
        assert!(html.contains("\"codeRepository\""), "{html}");
        assert!(html.contains("\"price\":\"0\""), "{html}");
        assert!(html.contains("\"@type\":\"BreadcrumbList\""), "{html}");
    }
}
