//! adi-css — the adi design system as compiled CSS.
//!
//! The source of truth is the SCSS under [`scss/`](../scss); `build.rs` compiles it (via
//! `grass`) into [`STYLESHEET`]. Two kinds of consumer share it:
//!
//! - **The wasm webapp** ([`adi-webapp`](../adi-webapp)) links the SCSS through Trunk, so
//!   the CSS lands in `<head>` with no flash of unstyled content.
//! - **Server-rendered pages** (e.g. [`adi-app`](../adi-app)'s placeholder / front-door
//!   pages) inline [`STYLESHEET`] directly, since they have no build step of their own.
//!
//! Both compile the same `scss/adi.scss`, so the look never drifts between them.

/// The compiled design system, ready to drop into a `<style>` element.
pub const STYLESHEET: &str = include_str!(concat!(env!("OUT_DIR"), "/adi.css"));

/// The stylesheet wrapped in a `<style>` tag, for inlining into an HTML `<head>`.
#[must_use]
pub fn style_tag() -> String {
    format!("<style>{STYLESHEET}</style>")
}
