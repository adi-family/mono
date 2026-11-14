//! Every address the panel sends a reader to that is **not** one of its own routes.
//!
//! [`crate::routing`] owns the inside of the app — `Route::Fleet.path()` and friends — and this
//! owns the outside: the docs, the donate page, the OAuth router a secret's login goes through.
//! They are here rather than beside the screens that use them because an outbound address is the
//! string in this crate most likely to rot without anything failing: a repository renamed, a doc
//! split in two, a heading reworded, and the only symptom is a reader landing on a 404 that
//! nobody who wrote the code will ever click. One file is a list somebody can actually check.
//!
//! What does **not** belong here: an API this app fetches (that is `fetch.rs`, and it is relative
//! to this origin), a service identifier that happens to look like a URL (an OAuth *scope*), or
//! anything under `/extended` (a route).

/// The docs site, as a whole — the "Docs" link in the chat's foot, where somebody is browsing
/// rather than asking one question.
pub(crate) const DOCS: &str = "https://docs.withadi.dev";

/// Where donating goes. adi runs on the operator's own machine and asks for nothing to do it, so
/// this is the one place it asks at all.
pub(crate) const DONATE: &str = "https://withadi.dev/mono-donate";

/// The OAuth router that runs a provider's flow and hands the token back in the redirect
/// fragment. Off this machine by necessity: a provider will not redirect to `app.adi`.
pub(crate) const OAUTH_ROUTER: &str = "https://oauth-router.withadi.dev";

/// What a *source* is on the sessions rail, and what merging one in does — the `?` on the source
/// menu's head.
pub(crate) const FLEET_SESSIONS: &str = "https://docs.withadi.dev/sessions/#more-than-one-machine";

/// What pointing the panel-wide source picker at another machine does — the `?` on the picker
/// menu's head.
pub(crate) const FLEET_PANEL_SOURCE: &str = "https://docs.withadi.dev/fleet/";

/// The onboarding welcome screen's "Getting started" link (ADI-MONO-157): opens in the browser,
/// which — unlike this panel — may already have accepted the site's cookie banner, so the site
/// can join this install to whatever brought that browser here (the download it clicked).
/// `install_id` is a random UUID, never anything that names this machine or its owner.
pub(crate) fn welcome_url(install_id: &str) -> String {
    format!("https://withadi.dev/welcome/?i={install_id}")
}
