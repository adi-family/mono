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
//! What it draws so far: a wallpaper ([`background`]), the apps on it as a home screen
//! ([`apps`], arranged by the person: [`arrange`], with websites added by hand: [`sites`]), the windows open over it ([`windows`] —
//! [`settings`], [`about`], a chat with an agent, [`chat`], one paired device's page, [`device`], and one app in a small browser,
//! [`browser`]), the top bar and island
//! ([`shell`], with the paired machines' list in [`sources`], both read from [`fleet`]), and the
//! `⌘K` palette ([`palette`]). A window's address
//! is only a place inside this document: every path that is not one of `main`'s other doors
//! mounts this screen, and [`windows::Desk`] reads the path itself.

mod about;
mod apps;
mod arrange;
mod background;
mod browser;
mod cache;
mod chat;
mod device;
mod fleet;
mod palette;
mod settings;
mod shell;
mod sites;
mod sources;
mod widgets;
mod windows;

use adi_ui::Lucide;
use leptos::{ev, prelude::*};

use crate::launcher::Action;
use crate::{icons, routing, ui};
use background::Appearance;
use palette::Item;
use shell::{Island, Shell, TopBar};
use windows::{Desk, Frame, Win};

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
    desk: Desk,
    home: arrange::Arrange,
    sites: sites::Sites,
) -> Vec<Item> {
    let now = wall.choice.get().appearance;
    let mut items = vec![
        Item::new(
            "Chat",
            "Open chat",
            "Talk to one of your agents",
            Lucide::MessageSquare,
            move || desk.open(Win::Chat),
        ),
        Item::new(
            "Settings",
            "Open settings",
            "Background and appearance",
            Lucide::Settings2,
            move || desk.open(Win::Settings),
        ),
        Item::new(
            "Home screen",
            "Edit home screen",
            "Move apps and widgets, resize or remove them",
            Lucide::LayoutGrid,
            move || home.edit(true),
        ),
        Item::new(
            "Home screen",
            "Add website",
            "Put a site any of your machines serves on the home screen",
            Lucide::Globe,
            move || sites.adding.set(true),
        ),
    ];
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

/// The new root screen: the wallpaper and the apps on it, whichever windows are open on it, the top bar and island,
/// and the `⌘K` palette —
/// which draws nothing until pressed, and always carries the way back out.
#[component]
pub(crate) fn NewUi() -> impl IntoView {
    let wall = background::Wallpaper::load();
    let desk = Desk::load();
    let shell = Shell::load();
    let fleet = fleet::Fleet::load();
    let apps = apps::Apps::load();
    let home = arrange::Arrange::load();
    let sites = sites::Sites::load();
    let palette_open = RwSignal::new(false);

    // Back and forward change the address without a reload; the window it names comes forward.
    let pop = window_event_listener(ev::popstate, move |_| desk.arrive(&routing::current_path()));
    on_cleanup(move || pop.remove());

    // Escape ends editing the home screen, or else closes the front window — unless it is closing
    // the palette, a modal dialog above every window that stops the key itself but is checked for
    // here too.
    let keys = window_event_listener(ev::keydown, move |ev| {
        let modal_open = document()
            .query_selector("[role=dialog][aria-modal=true]")
            .ok()
            .flatten()
            .is_some();
        if ev.key() != "Escape" || modal_open {
            return;
        }
        if home.editing.get_untracked() {
            home.edit(false);
        } else {
            desk.close_front();
        }
    });
    on_cleanup(move || keys.remove());

    // The floating surfaces — the palette and the windows — go light with the wallpaper.
    let light = Signal::derive(move || is_light(wall.choice.get().appearance));
    // The highest a window may go: the top bar's lower edge while it is drawn.
    let top_limit = Signal::derive(move || {
        if shell.layout.get().top_bar {
            shell::TOP_BAR_H
        } else {
            0.0
        }
    });

    view! {
        <div
            class="adi-new-root"
            class:is-dragging=move || desk.dragging.get()
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
            <apps::Home apps desk home sites light=light/>
            <windows::SnapPreview desk top=top_limit/>
            // Every window is drawn from this fixed list and stacked by `z-index`, never by
            // reordering: a window that moved in the DOM would be rebuilt, and lose whatever
            // was half-done inside it.
            {Win::FIXED
                .into_iter()
                .map(|w| view! {
                    <Show when=move || desk.is_open(w)>
                        <Frame win=w desk light=light top=top_limit>
                            {match w {
                                Win::Settings => {
                                    view! { <settings::Settings wall shell/> }.into_any()
                                }
                                Win::About => view! { <about::About/> }.into_any(),
                                Win::Chat => view! { <chat::Chat desk win=Win::Chat/> }.into_any(),
                                Win::Device => {
                                    view! { <device::Device fleet desk/> }.into_any()
                                }
                                // Not in the fixed list; drawn from the desk's lists below.
                                Win::App(_) | Win::Talk(_) => ().into_any(),
                            }}
                        </Frame>
                    </Show>
                })
                .collect_view()}
            // …and the apps' windows, one per open app, in the order they were opened — only
            // ever added to the end or taken out, so none already open moves, and none reloads
            // the page inside it.
            <For each=move || desk.apps.get() key=|(id, _)| *id let:app>
                <Frame win=Win::App(app.0) desk light=light top=top_limit>
                    <browser::Browser desk id=app.0/>
                </Frame>
            </For>
            // …and the chat windows beyond the first, the same way.
            <For each=move || desk.chats.get() key=|(id, _)| *id let:chat>
                <Frame win=Win::Talk(chat.0) desk light=light top=top_limit>
                    <chat::Chat desk win=Win::Talk(chat.0)/>
                </Frame>
            </For>
        </div>
        <Show when=move || shell.layout.get().top_bar>
            <TopBar desk fleet light=light/>
        </Show>
        <Island shell desk palette=palette_open light=light/>
        <sites::AddSite sites fleet light=light/>
        <palette::Palette items=move || commands(wall, desk, home, sites) light=light open=palette_open/>
    }
}
