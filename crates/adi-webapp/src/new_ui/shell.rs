//! The new UI's chrome over the wallpaper: a thin top bar, as iOS and macOS draw one, and the
//! island — a floating dock — on whichever edge the person puts it.
//!
//! Both are laid out from [`Layout`], a per-device preference kept in `localStorage` beside the
//! wallpaper, for the same reason.

use adi_ui::{Icon, IconSize, Lucide};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use super::windows::{Desk, Win};
use crate::{live, ui};

const KEY: &str = "adi-new-ui-layout";

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

/// The top bar: the time and date on the left, as an iPad's status bar has them, and on the right
/// whether the live channel to the stack is up.
#[component]
pub(super) fn TopBar(#[prop(into)] light: Signal<bool>) -> impl IntoView {
    let now = RwSignal::new(clock());
    let up = RwSignal::new(false);
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
        let connected = live::connected();
        if up.get_untracked() != connected {
            up.set(connected);
        }
    };
    set_timeout(check, std::time::Duration::ZERO);
    let tick = set_interval_with_handle(
        check,
        std::time::Duration::from_millis(CLOCK_TICK_MS.into()),
    );
    on_cleanup(move || {
        if let Ok(t) = tick {
            t.clear();
        }
    });

    view! {
        <header class="adi-new-top" class:light=move || light.get()>
            <span class="adi-new-top__time">{move || now.get().0}</span>
            <span class="adi-new-top__date">{move || now.get().1}</span>
            <span class="adi-new-top__spacer"></span>
            {move || {
                let (icon, label) = if up.get() {
                    (Lucide::Wifi, "Connected to the stack")
                } else {
                    (Lucide::WifiOff, "Not connected — reconnecting")
                };
                view! {
                    <span class="adi-new-top__status" title=label aria-label=label role="img">
                        <Icon icon=icon size=IconSize::Sm/>
                    </span>
                }
            }}
        </header>
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

/// The island: a floating dock of what can be opened from here, on the edge [`Layout`] names. A
/// small dot marks a window that is open, as macOS marks a running app.
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
        </nav>
    }
}
