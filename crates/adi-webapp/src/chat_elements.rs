//! The switch that puts the HTML chat (`<adi-chat>`, `design/elements/chat.js`) in the chat home's
//! centre in place of the Leptos transcript — per browser, off by default, while the element
//! catches up with everything the Leptos one does.
//!
//! `?chat-elements=1` / `?chat-elements=0` on any URL sets it, and ⌘K flips it ([`action`]). Only
//! an *open conversation* is drawn by the element; the home composer, the rail, the right panel and
//! terminal agents stay what they were, so the way back is always the same screen.

use crate::launcher::Action;
use crate::{icons, routing, ui};

/// Where the choice is remembered.
const KEY: &str = "adi-chat-elements";

/// The query parameter that sets it from a URL.
const PARAM: &str = "chat-elements";

/// Whether this browser draws conversations with `<adi-chat>`. A `?chat-elements=` in the address
/// bar wins, is saved, and is stripped, so a reload is an ordinary visit that remembers it.
pub(crate) fn enabled() -> bool {
    if let Some(v) = routing::query_param(PARAM) {
        store(v == "1");
        strip();
    }
    ui::storage()
        .and_then(|s| s.get_item(KEY).ok().flatten())
        .is_some_and(|v| v == "1")
}

/// Take this switch's parameter out of the address, and only it.
///
/// Not `routing::replace_state(current_path())`, which drops the whole query: `main` reads this
/// before `new_ui::enabled`, and `?new-ui=1&chat-elements=1` has to reach that one too.
fn strip() {
    let Some(window) = web_sys::window() else { return };
    let Ok(href) = window.location().href() else { return };
    let Ok(url) = web_sys::Url::new(&href) else { return };
    url.search_params().delete(PARAM);
    let path = format!("{}{}{}", url.pathname(), url.search(), url.hash());
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&path));
    }
}

fn store(on: bool) {
    if let Some(s) = ui::storage() {
        let _ = if on {
            s.set_item(KEY, "1")
        } else {
            s.remove_item(KEY)
        };
    }
}

/// The ⌘K row. Reloads, because the chat centre reads the switch when it is built.
pub(crate) fn action() -> Action {
    let (label, hint, on) = if enabled() {
        ("Use the Leptos chat", "Draw conversations the way they were", false)
    } else {
        ("Use the HTML chat", "Draw conversations with <adi-chat>", true)
    };
    Action::new(label, hint, icons::Icon::Elements, move || {
        store(on);
        let _ = web_sys::window().map(|w| w.location().reload());
    })
}
