//! Every address these pages send a reader to that is not one of their own.
//!
//! Kept in one file for the reason `adi-webapp`'s `links.rs` gives: an outbound URL is the string
//! most likely to rot with nothing failing — a repository renamed, a doc split in two — and the
//! only symptom is a reader landing on a 404 that nobody who wrote the code will ever click. On a
//! public page it is worse than in the panel, because the reader is a stranger and the click is
//! the one they were invited to make.

/// Where "Get adi" goes. The product's own page, which is the only honest next step for somebody
/// who has just read what an item does and has nothing to run it on.
pub const ADI: &str = "https://withadi.dev";

/// What a marketplace *is*, for the reader who wants the format rather than the apps — and for the
/// publisher who arrived here wondering how to get listed.
pub const MARKETPLACE_DOCS: &str =
    "https://github.com/adi-family/mono/blob/main/docs/marketplace.md";
