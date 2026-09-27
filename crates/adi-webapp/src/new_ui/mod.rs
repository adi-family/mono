//! The new UI: a second root screen, built up alongside the one it will replace.
//!
//! Opt-in, per browser. While it is on, the bare `/` mounts [`NewUi`] instead of the chat and
//! wizard ([`crate::Home`]); `/extended`, the marketplace and the embed are untouched. It is a
//! preference about how this person reads the app rather than a fact about the stack, so it lives
//! in `localStorage` — the same reasoning as [`crate::ui::advice_hidden`] — and never in the store.
//!
//! Two ways to flip it, because a screen under construction must never be one you cannot leave:
//!
//! * `⌘K` — [`action`] is a row in the old screens' menu, and the new screen's own palette
//!   ([`palette`]) always carries "Turn off new UI";
//! * `?new-ui=1` / `?new-ui=0` on any URL of the root document, read once by [`enabled`] and then
//!   taken back out of the address bar. The way out that needs no working wasm beyond this file.
//!
//! What it draws so far: a wallpaper ([`background`]) and, at `/settings`, the window that picks
//! it ([`settings`]). `/settings` is only a place inside this document — every path that is not
//! one of `main`'s other doors mounts this screen, and it reads the path itself.

mod background;
mod palette;
mod settings;

use adi_ui::Lucide;
use leptos::{ev, prelude::*};

use crate::launcher::Action;
use crate::{icons, routing, ui};
use background::Appearance;
use palette::Item;

/// Where the settings window opens.
const SETTINGS: &str = "/settings";

/// Where the choice is remembered.
const KEY: &str = "adi-new-ui";

/// The query parameter that sets the choice from a URL.
const PARAM: &str = "new-ui";

/// A link that turns the new UI on and lands on it — the old chat's door into it.
pub(crate) const TURN_ON: &str = "/?new-ui=1";

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

/// The palette's commands, as they stand right now.
fn commands(
    wall: background::Wallpaper,
    go: impl Fn(&'static str) + Copy + Send + Sync + 'static,
) -> Vec<Item> {
    let now = wall.choice.get().appearance;
    let mut items = vec![Item::new(
        "Settings",
        "Open settings",
        "Background and appearance",
        Lucide::Settings2,
        move || go(SETTINGS),
    )];
    // A row for each appearance the screen is not already fixed to — one while it is light or
    // dark, both while it follows the system.
    items.extend(
        [
            (Appearance::Light, "Light mode", Lucide::Sun),
            (Appearance::Dark, "Dark mode", Lucide::Moon),
        ]
        .into_iter()
        .filter(|(a, ..)| *a != now)
        .map(|(a, title, icon)| {
            Item::new("Appearance", title, "Wallpaper", icon, move || {
                wall.set(|c| c.appearance = a);
            })
        }),
    );
    items.push(Item::new(
        "New UI",
        "Turn off new UI",
        "Back to the current chat",
        Lucide::Sparkles,
        || set(false),
    ));
    items
}

/// Whether the screen is showing light right now — [`Appearance::Auto`] asks the system.
fn is_light(a: Appearance) -> bool {
    match a {
        Appearance::Light => true,
        Appearance::Dark => false,
        Appearance::Auto => window()
            .match_media("(prefers-color-scheme: light)")
            .ok()
            .flatten()
            .is_some_and(|m| m.matches()),
    }
}

/// The new root screen: the wallpaper, the settings window when the path asks for it, and the
/// `⌘K` palette — which draws nothing until pressed, and always carries the way back out.
#[component]
pub(crate) fn NewUi() -> impl IntoView {
    let wall = background::Wallpaper::load();
    let path = RwSignal::new(routing::current_path());

    // Back and forward move between `/` and `/settings` without a reload, so the browser's own
    // buttons need telling.
    let pop = window_event_listener(ev::popstate, move |_| path.set(routing::current_path()));
    on_cleanup(move || pop.remove());

    // The floating surfaces — the palette and the settings window — go light with the wallpaper.
    let light = Signal::derive(move || is_light(wall.choice.get().appearance));

    let go = move |to: &'static str| {
        routing::push_state(to);
        path.set(to.to_owned());
    };

    view! {
        <div
            class="adi-new-root"
            data-appearance=move || wall.choice.get().appearance.attr()
        >
            <div
                class="adi-new"
                class:adi-new--grain=move || wall.grain()
                style=move || wall.css()
            >
                <Show when=move || wall.photo().is_some()>
                    <div
                        class="adi-new__photo"
                        style=move || wall.photo().unwrap_or_default()
                    ></div>
                </Show>
            </div>
            <Show when=move || path.get() == SETTINGS>
                <settings::Window wall close=move || go("/") light=light/>
            </Show>
        </div>
        <palette::Palette
            items=move || commands(wall, go)
            light=light
        />
    }
}
