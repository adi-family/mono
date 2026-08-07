//! `adi-ui` — the adi component library.
//!
//! Leptos components, styled with Tailwind utilities that resolve to the
//! [`adi-css`](../adi-css) design tokens. A host page supplies the tokens (any page that
//! loads the adi design system already does) and imports
//! [`styles/ui.css`](./styles/ui.css); the components bring their own look from there.
//!
//! ```ignore
//! use adi_ui::{Button, ButtonVariant, Panel};
//!
//! view! {
//!     <Panel title="Ports">
//!         <Button variant=ButtonVariant::Primary on:click=reserve>"Reserve"</Button>
//!     </Panel>
//! }
//! ```
//!
//! # The one rule when writing a component
//!
//! **A class name has to appear in the source as a whole string literal.** Tailwind finds
//! classes by scanning this crate's `.rs` files for text that looks like a utility — it
//! never runs the code. So a `match` arm returning `"bg-accent text-on-accent"` is found,
//! and so is a class spelled out in a `view!`, but `format!("bg-{tone}")` is not: the
//! utility is simply never generated and the element renders unstyled. Build the whole
//! literal per branch, as [`ButtonVariant::classes`] does, rather than assembling one from
//! pieces.
//!
//! # Developing
//!
//! `trunk serve` in this directory opens the playground — every component on one page, in
//! both themes, with hot reload. See the [README](./README.md).

// Leptos components are PascalCase functions by convention, which is not a Rust function name.
#![allow(non_snake_case)]

mod badge;
mod button;
mod feedback;
mod field;
mod form;
mod input;
mod panel;
mod session;

pub use badge::{Badge, BadgeTone};
pub use button::{Button, ButtonSize, ButtonVariant};
pub use feedback::{Empty, Flash, FlashKind};
pub use field::Field;
pub use form::{Form, Hint};
pub use input::{Input, InputWidth, Select, Textarea};
pub use panel::Panel;
pub use session::{SessionGroup, SessionItem, SessionList, SessionRollup, SessionState};

/// Join a component's own classes with whatever the call site passed in `class`.
///
/// Every component here takes an optional `class` prop for the thing a component API can
/// never anticipate — a margin, a grid placement, a width. It lands last in the string,
/// which is also last in the cascade among equal-specificity utilities, so a caller's
/// `class="w-full"` beats the component's own width instead of losing to it at random.
///
/// `extra` arrives owned (a Leptos prop always does), so it is consumed and grown in place
/// rather than borrowed into a fresh allocation.
pub(crate) fn merge(own: &str, mut extra: String) -> String {
    if extra.is_empty() {
        return own.to_string();
    }
    extra.insert(0, ' ');
    extra.insert_str(0, own);
    extra
}
