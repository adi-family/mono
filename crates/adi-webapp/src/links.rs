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

/// A deep link into one section of one docs page.
///
/// GitHub renders a heading's anchor by lowercasing it, dropping punctuation and turning runs of
/// spaces into single hyphens — so `## 13. Driving a node's sessions from here` is reached at
/// `#13-driving-a-nodes-sessions-from-here`. Getting that wrong silently lands the reader at the
/// top of a thousand-line page, which for a `?` on a menu is barely better than no link at all,
/// so check a new one against the rendered page before shipping it.
///
/// A macro rather than a function so the result is still a `&'static str` and the repository
/// address is written once.
macro_rules! doc_section {
    ($page:literal, $anchor:literal) => {
        concat!(
            "https://github.com/adi-family/mono/blob/main/docs/",
            $page,
            "#",
            $anchor
        )
    };
}

/// The docs, as a directory — the "Docs" link in the chat's foot, where somebody is browsing
/// rather than asking one question.
pub(crate) const DOCS: &str = "https://github.com/adi-family/mono/tree/main/docs";

/// Where donating goes. adi runs on the operator's own machine and asks for nothing to do it, so
/// this is the one place it asks at all.
pub(crate) const DONATE: &str = "https://withadi.dev/mono-donate";

/// The OAuth router that runs a provider's flow and hands the token back in the redirect
/// fragment. Off this machine by necessity: a provider will not redirect to `app.adi`.
pub(crate) const OAUTH_ROUTER: &str = "https://oauth-router.withadi.dev";

/// What a *source* is on the sessions rail, and what merging one in does — the `?` on the source
/// menu's head (`docs/fleet.md` §13).
pub(crate) const FLEET_SESSIONS: &str =
    doc_section!("fleet.md", "13-driving-a-nodes-sessions-from-here");
