//! The two files that are addressed to a crawler rather than to a person.
//!
//! Neither is decoration. A marketplace's whole reason for having public pages is that somebody
//! searching for "adi crm dashboard" can land on one, and a sitemap is how a site with no inbound
//! links gets read at all: every item page is one hop from the shelf, but nothing links to the
//! shelf on the day it is published.

use std::fmt::Write as _;

use crate::html::escape;
use crate::{Site, Source};

/// `robots.txt`: everything is public, and here is the map.
///
/// Written even for a site with no base URL — a `robots.txt` that allows everything is the
/// default a crawler assumes anyway, and publishing it explicitly is what stops a host's own
/// placeholder from standing in for it.
#[must_use]
pub fn robots(site: &Site) -> String {
    let sitemap = site.url("/sitemap.xml");
    if sitemap.is_empty() {
        return "User-agent: *\nAllow: /\n".to_string();
    }
    format!("User-agent: *\nAllow: /\n\nSitemap: {sitemap}\n")
}

/// `sitemap.xml`: the shelf and every item, in the order they are published.
///
/// `None` when the site has no base URL, because every URL in a sitemap is absolute by the
/// specification and a relative one is not a smaller sitemap, it is an invalid one.
///
/// No `lastmod`: the manifest carries no date, and the generation time would say only when this
/// command last ran — which is exactly the signal `lastmod` is not supposed to be.
#[must_use]
pub fn sitemap(site: &Site, sources: &[Source]) -> Option<String> {
    let home = site.url("/");
    if home.is_empty() {
        return None;
    }
    // The shelf, then the page somebody who has not got adi is sent to, then the items. All three
    // kinds are pages a search result can land on.
    let mut urls = vec![home, site.url("/get/")];
    for source in sources {
        for entry in &source.manifest.bundles {
            urls.push(site.item_url(&source.name, entry));
        }
    }
    let mut body = String::new();
    for url in urls {
        let _ = writeln!(body, "  <url><loc>{}</loc></url>", escape(&url));
    }
    Some(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n{body}</urlset>\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{fixture_site, fixture_source};

    #[test]
    fn the_sitemap_lists_the_shelf_and_every_item_absolutely() {
        let xml = sitemap(&fixture_site(), &[fixture_source()]).expect("a base url");
        assert!(xml.contains("<loc>https://market.example.com/</loc>"), "{xml}");
        assert!(xml.contains("<loc>https://market.example.com/local/crm-suite/</loc>"), "{xml}");
        assert!(xml.contains("<loc>https://market.example.com/get/</loc>"), "{xml}");
        assert_eq!(xml.matches("<url>").count(), 4, "the shelf, get, and two entries: {xml}");
    }

    #[test]
    fn robots_points_at_the_sitemap_when_there_is_one() {
        assert!(robots(&fixture_site()).contains("Sitemap: https://market.example.com/sitemap.xml"));
        let nowhere = Site {
            name: "x".to_string(),
            base_url: String::new(),
        };
        assert!(!robots(&nowhere).contains("Sitemap:"));
    }
}
