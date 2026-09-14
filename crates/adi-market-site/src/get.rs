//! "Get adi" — the page a reader lands on when the answer to *how do I run this* is that they do
//! not have adi yet, and the two-state block every other page's action is built from.
//!
//! **A page cannot look at somebody's disk.** `http://app.adi` cannot be fetched from an https
//! page (mixed content), adi-app refuses an `/api` request whose `Origin` is not its own `Host`
//! anyway, and there is no `adi://` scheme registered to deep-link into and time out on. A guess
//! from the user agent would be a guess. So the site does the honest version of the check: it
//! carries **both answers in the markup**, leads with the one that is right for a stranger, and
//! asks once — [`assets/site.js`] remembers which of the two a reader said they were, for every
//! page of the site from then on.
//!
//! With no script at all, both are on the page: the download button, and under it the commands.
//! That is the order a first-time reader wants anyway, which is why it is also the default.

// The same shape as `item.rs`: a handful of values mapped into markup and collected. See the note
// there for why `clippy::format_collect`'s fold-with-a-`write!` is not an improvement here.
#![allow(clippy::format_collect)]

use crate::html::escape;
use crate::icons::Icon;
use crate::shell::{self, Head};
use crate::{Site, Source, links};

/// The page's own address, from `depth` levels down.
#[must_use]
pub fn href(up: &str, from: Option<&str>) -> String {
    match from {
        Some(address) => format!("{up}get/?from={}", escape(address)),
        None => format!("{up}get/"),
    }
}

/// Somebody who already has adi, telling the site so — and the way back out of having said it.
///
/// Both are `js-only`: with no script these would be buttons that do nothing, and a dead control
/// is worse than no control. The way back is `only-adi` as well, because until the question has
/// been answered there is nothing to take back, and offering both at once is a page asking the
/// reader to choose a setting rather than get on with it.
#[must_use]
pub fn toggle(has_adi: bool) -> String {
    let (value, label, extra) = if has_adi {
        ("", "I do not have adi on this machine", " only-adi")
    } else {
        ("yes", "I already have adi", "")
    };
    format!(
        "<button class=\"linky js-only{extra}\" type=\"button\" \
         data-adi-set=\"{value}\">{label}</button>"
    )
}

/// One numbered step: what it is for, and the line that does it.
#[must_use]
pub fn step(label: &str, command: Option<&str>, attribute: &str) -> String {
    let command = command.map_or_else(String::new, |command| {
        format!(
            "<p class=\"cmd\"><span class=\"prompt\">$</span><code{attribute}>{}</code></p>",
            escape(command)
        )
    });
    format!(
        "<li><div class=\"step\"><p class=\"label\">{}</p>{command}</div></li>",
        escape(label)
    )
}

/// The command that adds one marketplace to a reader's own store — the step everything else here
/// depends on, and the one the first version of this site left out. Without it
/// `marketplace install <name>/<slug>` names a marketplace the machine has never heard of.
#[must_use]
pub fn add_command(source: &Source) -> Option<String> {
    let url = source.url.as_deref()?;
    Some(format!("adi-mono marketplace add {} {url}", source.name))
}

/// The page.
#[must_use]
pub fn render(site: &Site, sources: &[Source]) -> String {
    let adds = sources
        .iter()
        .filter_map(|source| {
            add_command(source).map(|command| {
                step(
                    &format!("Add the {} marketplace \u{2014} once, on this machine", source.title()),
                    Some(&command),
                    "",
                )
            })
        })
        .collect::<String>();
    let body = format!(
        "<div class=\"wrap\"><article class=\"item\">\
         <a class=\"back\" href=\"./\">{back}All apps</a>\
         <header class=\"get__head\">\
         <h1>Get adi</h1>\
         <p class=\"lede\">It runs on your own machine \u{2014} one file, and no account to create \
         before or after. Installing it gives you a control panel at <code>app.adi</code>, and \
         everything in the store installs into that.</p>\
         </header>\
         <section class=\"section\"><h2>Download</h2><ul class=\"dl\">{downloads}</ul>\
         <p class=\"note-after\">Always the newest release. Free while in beta.</p></section>\
         <section class=\"section\"><h2>Then, from a shell</h2><ol class=\"steps\">\
         {adds}\
         {install}\
         </ol>{back_to}</section>\
         <p class=\"get__ask\"><span class=\"if-no-adi\">{ask_yes}</span>{ask_no}</p>\
         </article></div>",
        back = Icon::ArrowLeft.svg("i--sm"),
        downloads = downloads(),
        install = step(
            "Install what you came for",
            Some("adi-mono marketplace install <marketplace>/<app>"),
            " data-from=\"install\"",
        ),
        back_to = "<a class=\"btn btn--primary\" data-from=\"back\" href=\"./\" hidden>\
                   Back to what you were looking at</a>",
        ask_yes = toggle(false),
        ask_no = toggle(true),
    );
    shell::document(site, &head(site), 1, &body)
}

/// The three files, as the landing lists them (`adi-landing` block 10): rows with hairlines rather
/// than three buttons, because a list of files is a list — and the file type beside each name is
/// the one machine string on the row, so it is the one mono.
fn downloads() -> String {
    links::DOWNLOADS
        .into_iter()
        .map(|(os, asset, note)| {
            format!(
                "<li><a href=\"{href}\">Download for {os}<span class=\"mono\">{note}</span></a></li>",
                href = escape(&links::download(asset)),
            )
        })
        .collect::<String>()
}

/// This page is about the product, not about one item, so its description says what downloading
/// does rather than what any bundle is.
fn head(site: &Site) -> Head {
    let description = "adi runs agents, tools and dashboards on your own machine. Download it for \
                       macOS, Linux or Windows, then install anything published here with one \
                       line \u{2014} no account, and nothing runs until you start it.";
    Head {
        title: format!("Get adi \u{2014} {}", site.name),
        description: description.to_string(),
        canonical: site.url("/get/"),
        image: None,
        data: vec![serde_json::json!({
            "@context": "https://schema.org",
            "@type": "SoftwareApplication",
            "name": "adi",
            "description": description,
            "url": links::ADI,
            "applicationCategory": "DeveloperApplication",
            "operatingSystem": "macOS, Linux, Windows",
            "isAccessibleForFree": true,
            "offers": { "@type": "Offer", "price": "0", "priceCurrency": "USD" },
        })],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{fixture_site, fixture_source};

    #[test]
    fn the_page_names_every_platform_and_the_release_that_never_goes_stale() {
        let html = render(&fixture_site(), &[fixture_source()]);
        for (os, asset, _) in links::DOWNLOADS {
            assert!(html.contains(&format!("Download for {os}")), "{html}");
            assert!(html.contains(&links::download(asset)), "{html}");
        }
        assert!(!html.contains("ADI-windows-x64.zip"), "the zip is the updater's, not a download");
        assert!(html.contains("releases/latest/download"), "never a pinned version: {html}");
    }

    /// The step the first version of this site left out: an install names a marketplace, and a
    /// machine that has not added it has never heard of it.
    #[test]
    fn adding_the_marketplace_comes_before_installing_from_it() {
        let html = render(&fixture_site(), &[fixture_source()]);
        // The whole command, not "marketplace install": prose about installing sits well above
        // either step, and a looser needle finds a sentence rather than a command.
        let add = html.find("adi-mono marketplace add local").expect("the add line");
        let install = html.find("adi-mono marketplace install").expect("the install line");
        assert!(add < install, "add comes first: {html}");
    }

    #[test]
    fn a_marketplace_with_no_published_url_offers_no_add_line_to_guess_at() {
        let mut source = fixture_source();
        source.url = None;
        let html = render(&fixture_site(), &[source]);
        assert!(!html.contains("adi-mono marketplace add"), "{html}");
    }

    #[test]
    fn the_control_that_asks_is_never_a_dead_button_without_script() {
        assert!(toggle(false).contains("js-only"), "{}", toggle(false));
        assert!(toggle(true).contains("data-adi-set=\"\""), "{}", toggle(true));
    }
}
