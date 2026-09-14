//! Every address these pages send a reader to that is not one of their own.
//!
//! Kept in one file for the reason `adi-webapp`'s `links.rs` gives: an outbound URL is the string
//! most likely to rot with nothing failing — a repository renamed, a doc split in two — and the
//! only symptom is a reader landing on a 404 that nobody who wrote the code will ever click. On a
//! public page it is worse than in the panel, because the reader is a stranger and the click is
//! the one they were invited to make.

/// The product's own page.
pub const ADI: &str = "https://withadi.dev";

/// What a marketplace *is*, for the reader who wants the format rather than the apps — and for the
/// publisher who arrived here wondering how to get listed.
pub const MARKETPLACE_DOCS: &str =
    "https://github.com/adi-family/mono/blob/main/docs/marketplace.md";

/// The control panel on the reader's **own** machine, behind the front door adi installs.
///
/// A link rather than anything this page can check: a top-level navigation from https to
/// `http://app.adi` is allowed, but *fetching* it is not (mixed content), and adi-app refuses an
/// `/api` request whose `Origin` is not its own `Host` anyway (`crates/adi-app/src/origin.rs`).
/// So the site can offer to open the panel; it can never quietly find out whether one is there.
/// See [`crate::get`] for what is done instead.
pub const PANEL: &str = "http://app.adi";

/// The downloads, as the landing publishes them (`adi-landing`, block 10). `releases/latest`
/// redirects to whatever the newest release carries, so nothing here names a version and nothing
/// here goes stale on its own.
///
/// The suffixes come from the asset filenames, which is the only thing actually known: the Linux
/// and Windows assets say `x64` and `ADI.dmg` says nothing, so the macOS row claims nothing.
/// Windows is the **installer**, never `ADI-windows-x64.zip` — that file exists for the
/// self-updater and a person downloading a zip of loose binaries is a worse first five minutes
/// (`docs/adi-update.md`).
pub const DOWNLOADS: [(&str, &str, &str); 3] = [
    ("macOS", "ADI.dmg", ".dmg"),
    ("Linux", "adi-linux-x64.tar.gz", "x64 · .tar.gz"),
    ("Windows", "ADI-Setup-x64.exe", "x64 · .exe"),
];

/// One release asset's URL.
#[must_use]
pub fn download(asset: &str) -> String {
    format!("https://github.com/adi-family/mono/releases/latest/download/{asset}")
}

/// One item's page **in the reader's own panel** — where the real Install button is, with the
/// dialog that asks which project it goes into.
#[must_use]
pub fn panel_item(marketplace: &str, slug: &str) -> String {
    format!("{PANEL}/marketplace/{marketplace}/{slug}")
}

/// The panel's marketplace listing.
#[must_use]
pub fn panel_market() -> String {
    format!("{PANEL}/marketplace")
}
