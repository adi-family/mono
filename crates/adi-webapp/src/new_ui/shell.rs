//! The new UI's chrome over the wallpaper: a thin top bar, as macOS and iOS draw one, and the
//! island — a floating dock — on whichever edge the person puts it.
//!
//! Both are laid out from [`Layout`], a per-device preference kept in `localStorage` beside the
//! wallpaper, for the same reason.

use adi_ui::{Icon, IconSize, Lucide, Mark};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use super::sources::Sources;
use super::windows::{Desk, Win};
use crate::{live, ui};

const KEY: &str = "adi-new-ui-layout";

/// The top bar's height in CSS pixels — `$top-bar-h` in `_new_ui.scss`, which it must match:
/// windows stop at this line.
pub(super) const TOP_BAR_H: f64 = 28.0;

/// How often the top bar looks at the time and the connection. Once a second: the socket opens a
/// moment after the page does, and a slower look would say "not connected" for that long.
const CLOCK_TICK_MS: u32 = 1_000;

/// An edge of the screen.
#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Edge {
    Top,
    Left,
    #[default]
    Bottom,
    Right,
}

impl Edge {
    pub(super) const ALL: [(Self, &'static str); 4] = [
        (Self::Top, "Top"),
        (Self::Left, "Left"),
        (Self::Bottom, "Bottom"),
        (Self::Right, "Right"),
    ];

    fn attr(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Left => "left",
            Self::Bottom => "bottom",
            Self::Right => "right",
        }
    }
}

/// Where the chrome goes.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Layout {
    pub(super) island: Edge,
    pub(super) top_bar: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            island: Edge::Bottom,
            top_bar: true,
        }
    }
}

/// The layout's live state, shared by the screen and the settings window.
#[derive(Clone, Copy)]
pub(super) struct Shell {
    pub(super) layout: RwSignal<Layout>,
}

impl Shell {
    pub(super) fn load() -> Self {
        let layout = ui::storage()
            .and_then(|s| s.get_item(KEY).ok().flatten())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Self {
            layout: RwSignal::new(layout),
        }
    }

    /// Change the layout and save it.
    pub(super) fn set(self, change: impl FnOnce(&mut Layout)) {
        self.layout.update(change);
        if let (Some(s), Ok(json)) = (
            ui::storage(),
            serde_json::to_string(&self.layout.get_untracked()),
        ) {
            let _ = s.set_item(KEY, &json);
        }
    }
}

/// The top bar, as macOS lays out its menu bar: the mark on the left; on the right the status —
/// how many of the paired machines are active, whether the stack's live channel is up, whether
/// this device is online — and then the time and the date.
#[component]
pub(super) fn TopBar(
    desk: Desk,
    fleet: super::fleet::Fleet,
    #[prop(into)] light: Signal<bool>,
) -> impl IntoView {
    let now = RwSignal::new(clock());
    let stack = RwSignal::new(false);
    let online = RwSignal::new(window().navigator().on_line());

    // The live channel's state is read here, from a timer, and never from the view. The flag in
    // `live` is a signal made on first read and owned by whatever reactive scope is current at
    // that moment: first read from inside a view, it belongs to that view and is thrown away the
    // first time the view re-runs — after which it never reports a connection again. A timer
    // callback runs outside every scope, which is where the old screens read it from too.
    let check = move || {
        let c = clock();
        if now.with_untracked(|n| *n != c) {
            now.set(c);
        }
        let up = live::connected();
        if stack.get_untracked() != up {
            stack.set(up);
        }
    };
    set_timeout(check, std::time::Duration::ZERO);
    let tick = set_interval_with_handle(
        check,
        std::time::Duration::from_millis(CLOCK_TICK_MS.into()),
    );

    // The browser says when the network comes and goes; asking it on a timer would only lag.
    let went_on = window_event_listener(leptos::ev::online, move |_| online.set(true));
    let went_off = window_event_listener(leptos::ev::offline, move |_| online.set(false));

    on_cleanup(move || {
        if let Ok(t) = tick {
            t.clear();
        }
        went_on.remove();
        went_off.remove();
    });

    view! {
        <header class="adi-new-top" class:light=move || light.get()>
            // The mark is the way to "About adi", as the Apple menu is to About This Mac.
            <button
                class="adi-new-top__logo"
                type="button"
                title="About adi"
                aria-label="About adi"
                on:click=move |_| desk.open(Win::About)
            >
                <Mark class="adi-new-top__mark"/>
            </button>
            <span class="adi-new-top__spacer"></span>
            <Sources fleet desk light=light/>
            {move || status(
                stack.get(),
                (Lucide::Plug, "Connected to the stack"),
                (Lucide::Unplug, "Not connected to the stack — reconnecting"),
            )}
            {move || status(
                online.get(),
                (Lucide::Wifi, "Online"),
                (Lucide::WifiOff, "Offline — no internet connection"),
            )}
            {match calendar_href() {
                Some(href) => view! {
                    // A real link, not a script: a browser asks before handing a scheme to an
                    // app, and the Mac app passes a clicked link it does not serve to macOS
                    // (`WebPanel.swift`), which opens it in Calendar the same way.
                    <a class="adi-new-top__clock" href=href title="Open Calendar">
                        <span class="adi-new-top__time">{move || now.get().0}</span>
                        <span class="adi-new-top__date">{move || now.get().1}</span>
                    </a>
                }
                .into_any(),
                None => view! {
                    <span class="adi-new-top__clock">
                        <span class="adi-new-top__time">{move || now.get().0}</span>
                        <span class="adi-new-top__date">{move || now.get().1}</span>
                    </span>
                }
                .into_any(),
            }}
        </header>
    }
}

/// The address that opens this device's own calendar app, or `None` where there is no such thing
/// to point at — Linux has no one calendar and no scheme that reaches whichever is installed.
///
/// * macOS — `ical:`, which Calendar registers (checked with `LSCopyDefaultHandlerForURLScheme`);
/// * iOS and iPadOS — `calshow:`, the scheme Calendar answers there. iPadOS reports itself as a
///   Mac in its user agent, so a "Mac" with a touch screen is taken to be an iPad;
/// * Windows — `outlookcal:`, the Calendar app's (and the new Outlook's) scheme;
/// * Android — an intent for whichever app holds the calendar category, the only way a page can
///   ask for "the calendar" rather than one vendor's.
fn calendar_href() -> Option<&'static str> {
    let nav = window().navigator();
    let ua = nav.user_agent().unwrap_or_default();
    let touch = nav.max_touch_points() > 1;
    if ua.contains("iPhone") || ua.contains("iPad") || (ua.contains("Macintosh") && touch) {
        Some("calshow://")
    } else if ua.contains("Android") {
        Some(
            "intent:#Intent;action=android.intent.action.MAIN;\
             category=android.intent.category.APP_CALENDAR;end",
        )
    } else if ua.contains("Macintosh") {
        Some("ical://")
    } else if ua.contains("Windows") {
        Some("outlookcal:")
    } else {
        None
    }
}

/// One status icon: which of two it shows, and what it says on hover.
fn status(on: bool, yes: (Lucide, &'static str), no: (Lucide, &'static str)) -> impl IntoView {
    let (icon, label) = if on { yes } else { no };
    view! {
        <span
            class="adi-new-top__status"
            class:is-off=!on
            title=label
            aria-label=label
            role="img"
        >
            <Icon icon=icon size=IconSize::Sm/>
        </span>
    }
}

/// The time and the date, in this browser's language: `9:41` and `Sat 27 Sep`, or whatever the
/// locale writes instead.
fn clock() -> (String, String) {
    let d = js_sys::Date::new_0();
    let lang = window()
        .navigator()
        .language()
        .unwrap_or_else(|| "en".into());
    let opts = |pairs: &[(&str, &str)]| {
        let o = js_sys::Object::new();
        for (k, v) in pairs {
            let _ = js_sys::Reflect::set(&o, &(*k).into(), &(*v).into());
        }
        o
    };
    let time = d.to_locale_time_string_with_options(
        &lang,
        &opts(&[("hour", "numeric"), ("minute", "2-digit")]),
    );
    let date = d.to_locale_date_string(
        &lang,
        &opts(&[("weekday", "short"), ("day", "numeric"), ("month", "short")]),
    );
    (time.into(), date.into())
}

/// The island: a floating dock of what can be opened from here, and of the apps open in windows,
/// on the edge [`Layout`] names. A small dot marks a window that is open, as macOS marks a
/// running app.
#[component]
pub(super) fn Island(
    shell: Shell,
    desk: Desk,
    /// The palette's, so the search button opens it.
    palette: RwSignal<bool>,
    #[prop(into)] light: Signal<bool>,
) -> impl IntoView {
    view! {
        <nav
            class="adi-new-island"
            class:light=move || light.get()
            class:under-top-bar=move || shell.layout.get().top_bar
            data-edge=move || shell.layout.get().island.attr()
            aria-label="Island"
        >
            <button
                class="adi-new-island__item"
                type="button"
                title="Search"
                aria-label="Search"
                on:click=move |_| palette.set(true)
            >
                <Icon icon=Lucide::Search size=IconSize::Lg/>
            </button>
            <button
                class="adi-new-island__item"
                class:is-open=move || desk.is_open(Win::Settings)
                type="button"
                title=Win::Settings.title()
                aria-label=Win::Settings.title()
                on:click=move |_| desk.open(Win::Settings)
            >
                <Icon icon=Lucide::Settings2 size=IconSize::Lg/>
            </button>
            <button
                class="adi-new-island__item"
                class:is-open=move || desk.is_open(Win::Chat)
                type="button"
                title=Win::Chat.title()
                aria-label=Win::Chat.title()
                on:click=move |_| desk.open(Win::Chat)
            >
                <Icon icon=Lucide::MessageSquare size=IconSize::Lg/>
            </button>
            // The apps open in windows, after a divider, in the order they were opened — as the
            // Dock lists running apps after its own. Each carries the open dot; pressing one
            // brings its window forward.
            <Show when=move || desk.apps.with(|a| !a.is_empty())>
                <span class="adi-new-island__divider" aria-hidden="true"></span>
            </Show>
            <For each=move || desk.apps.get() key=|(id, _)| *id let:app>
                {
                    let (id, app) = app;
                    let win = Win::App(id);
                    let label = match &app.machine {
                        Some(m) => format!("{} on {m}", app.name),
                        None => app.name.clone(),
                    };
                    view! {
                        <button
                            class="adi-new-island__item is-open"
                            class:is-front=move || desk.is_front(win)
                            type="button"
                            title=label.clone()
                            aria-label=label
                            on:click=move |_| desk.focus(win)
                        >
                            <super::apps::AppMark
                                name=app.name.clone()
                                favicon=app.favicon.clone()
                                icon=app.icon.clone()
                            />
                        </button>
                    }
                }
            </For>
        </nav>
    }
}
