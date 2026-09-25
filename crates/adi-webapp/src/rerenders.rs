//! Showing rerenders: an overlay that flashes whatever part of the page the DOM just changed.
//!
//! The overlay is [`rerenders.js`](../rerenders.js), which `index.html` loads ahead of the wasm
//! bundle — so when it was left on, it is already watching as the app mounts. It watches the DOM
//! with a `MutationObserver` rather than anything inside Leptos, because the DOM is the one place a
//! rerender can be seen here: a component runs once, and what reruns afterwards is whichever
//! closure a signal woke. A solid box is a subtree rebuilt, a dashed one a text node or attribute
//! patched in place.
//!
//! This module is the Rust half of the bridge (`window.__adiRerenders`), as [`crate::pwa`] is for
//! installing: [`action`] is the ⌘K row that flips it. The switch itself, and the `localStorage`
//! key that remembers it, belong to the script, which has to read them before there is any wasm
//! to ask.

use js_sys::{Function, Reflect};
use wasm_bindgen::{JsCast as _, JsValue};

use crate::icons;
use crate::launcher::Action;

/// The `window.__adiRerenders` bridge object, or `None` when the script didn't load.
fn bridge() -> Option<JsValue> {
    let window = JsValue::from(web_sys::window()?);
    let bridge = Reflect::get(&window, &JsValue::from_str("__adiRerenders")).ok()?;
    (!bridge.is_undefined() && !bridge.is_null()).then_some(bridge)
}

/// Flip the overlay. A no-op when the script didn't load.
fn toggle() {
    let Some(bridge) = bridge() else {
        return;
    };
    if let Ok(f) = Reflect::get(&bridge, &JsValue::from_str("toggle"))
        && let Some(f) = f.dyn_ref::<Function>()
    {
        let _ = f.call0(&bridge);
    }
}

/// The ⌘K row, named for what pressing it does — "Show" while the overlay is off, "Hide" while it
/// is on. Read fresh on every draw of the menu, which is what keeps the label true after a toggle.
///
/// `None` when the script didn't load: a row that flips nothing is worse than no row.
pub(crate) fn action() -> Option<Action> {
    let bridge = bridge()?;
    let on = Reflect::get(&bridge, &JsValue::from_str("on"))
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some(if on {
        Action::new(
            "Hide rerenders",
            "Stop flashing DOM changes",
            icons::Icon::Rerenders,
            toggle,
        )
    } else {
        Action::new(
            "Show rerenders",
            // Carries the words somebody would type looking for it — "debug", "highlight",
            // "render" — as well as saying what the boxes will be.
            "Debug: highlight every DOM rebuild and patch",
            icons::Icon::Rerenders,
            toggle,
        )
    })
}
