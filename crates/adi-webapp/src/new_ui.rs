//! The new UI: a second root screen, built up alongside the one it will replace.
//!
//! Opt-in, per browser. While it is on, the bare `/` mounts [`NewUi`] instead of the chat and
//! wizard ([`crate::Home`]); `/extended`, the marketplace and the embed are untouched. It is a
//! preference about how this person reads the app rather than a fact about the stack, so it lives
//! in `localStorage` — the same reasoning as [`crate::ui::advice_hidden`] — and never in the store.
//!
//! Two ways to flip it, because a screen under construction must never be one you cannot leave:
//!
//! * the `⌘K` menu — [`action`] is a row on every shell, and the new screen mounts the menu
//!   (with that one row) even though it draws nothing else;
//! * `?new-ui=1` / `?new-ui=0` on any URL of the root document, read once by [`enabled`] and then
//!   taken back out of the address bar. The way out that needs no working wasm beyond this file.

use leptos::prelude::*;

use crate::launcher::{self, Action, Launcher};
use crate::{icons, routing, ui};

/// Where the choice is remembered.
const KEY: &str = "adi-new-ui";

/// The query parameter that sets the choice from a URL.
const PARAM: &str = "new-ui";

/// Whether this browser has the new UI on. A `?new-ui=` in the address bar wins, is saved, and is
/// stripped — so a reload afterwards is an ordinary visit that remembers the choice.
pub(crate) fn enabled() -> bool {
    if let Some(v) = routing::query_param(PARAM) {
        store(v == "1");
        routing::replace_state(&routing::current_path());
    }
    ui::storage()
        .and_then(|s| s.get_item(KEY).ok().flatten())
        .is_some_and(|v| v == "1")
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

/// Flip the choice and reload onto the root, which is the one document the choice decides.
fn set(on: bool) {
    store(on);
    let _ = window().location().set_href("/");
}

/// The menu row that flips it — "turn on" everywhere but the new screen, "turn off" on it.
pub(crate) fn action() -> Action {
    if enabled() {
        Action::new(
            "Turn off new UI",
            "Back to the current chat",
            icons::Icon::Spark,
            || set(false),
        )
    } else {
        Action::new(
            "Turn on new UI",
            "Preview the screen in progress",
            icons::Icon::Spark,
            || set(true),
        )
    }
}

/// The new root screen. For now the background and nothing else — the `⌘K` menu is mounted but
/// draws nothing until pressed, and it offers only the way back out.
#[component]
pub(crate) fn NewUi() -> impl IntoView {
    let launcher = Launcher::new();
    view! {
        <div class="adi-new"></div>
        {launcher::overlay(launcher, || vec![action()])}
    }
}
