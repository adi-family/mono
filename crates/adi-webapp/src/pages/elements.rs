//! The UI elements page — the adi-elements gallery, inside the panel.
//!
//! There is nothing else in this module on purpose. The page is one custom element,
//! `<adi-gallery>`, defined in `design/elements/gallery.js` and loaded by the module script in
//! `index.html`; everything on the screen — the sections, the specimens, the markup blocks — is
//! built by that element in JavaScript. Leptos only puts the tag in the document.
//!
//! That this works at all with no bridge is the point of the thing: a custom element is an HTML
//! tag, so it upgrades whenever it lands in the document, whoever put it there. The same
//! `<adi-button variant="primary">` works in a `view!` here, in a server-rendered front-door
//! page, and in a file opened straight off disk.

use leptos::prelude::*;

/// The gallery page (`/extended/ui`).
pub(crate) fn elements_view() -> AnyView {
    view! { <adi-gallery></adi-gallery> }.into_any()
}
