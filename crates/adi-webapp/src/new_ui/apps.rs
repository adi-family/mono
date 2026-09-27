//! The apps on the wallpaper, as iOS lays out a home screen: a tile for each of this machine's
//! dashboards, its name beneath, in rows from the top.
//!
//! What a tile opens follows the old chat's Apps rail (`chat_dash_item`), through the same two
//! helpers: a running dashboard opens where it answers ([`dashboards::open_url`]); a stopped one
//! with a routable host is still a link, to the address that wakes it ([`dashboards::wake_url`]),
//! because dashboards stop when idle and start on the first request; one with neither is a tile
//! with nothing to open, dimmed.
//!
//! This machine's dashboards only, for now — a paired machine's need a password and a grant
//! before they are something to open, and the rail's controls for those have no place here yet.

use adi_ui::{Icon, IconSize, Lucide};
use adi_webapp_api::types::Dashboard;
use leptos::prelude::*;

use crate::fetch;
use crate::pages::dashboards;

/// How often the dashboards are read again. One started or stopped elsewhere shows up within
/// this; nothing on this screen changes them yet.
const TICK_MS: u32 = 30_000;

/// This machine's dashboards, archived ones left out. `None` until the first answer.
#[derive(Clone, Copy)]
pub(super) struct Apps {
    list: RwSignal<Option<Vec<Dashboard>>>,
}

impl Apps {
    /// Read now, and again every [`TICK_MS`] for as long as the calling scope lives.
    pub(super) fn load() -> Self {
        let apps = Self {
            list: RwSignal::new(None),
        };
        apps.read();
        let tick = set_interval_with_handle(
            move || apps.read(),
            std::time::Duration::from_millis(TICK_MS.into()),
        );
        on_cleanup(move || {
            if let Ok(t) = tick {
                t.clear();
            }
        });
        apps
    }

    fn read(self) {
        leptos::task::spawn_local(async move {
            // A failed read keeps the last answer: stale by a tick beats an emptied screen.
            if let Ok(state) = fetch::dashboards().await {
                let live = state
                    .dashboards
                    .into_iter()
                    .filter(|d| !d.is_archived())
                    .collect();
                self.list.set(Some(live));
            }
        });
    }
}

/// The grid. Draws nothing until there is an app, so a machine with none keeps a bare wallpaper.
#[component]
pub(super) fn Home(apps: Apps, #[prop(into)] light: Signal<bool>) -> impl IntoView {
    let has_any = move || apps.list.with(|l| l.as_ref().is_some_and(|l| !l.is_empty()));
    view! {
        <Show when=has_any>
            <nav class="adi-new-apps" class:light=move || light.get() aria-label="Apps">
                // Keyed on the id and everything a tile is drawn from, so a dashboard that starts
                // or stops is redrawn with its new link rather than keeping the old one.
                <For
                    each=move || apps.list.get().unwrap_or_default()
                    key=|d| (d.id.clone(), d.name.clone(), d.host.clone(), d.frontend_running)
                    let:d
                >
                    {tile(&d)}
                </For>
            </nav>
        </Show>
    }
}

/// One app: a link to wherever it answers or wakes, or a dimmed tile when there is nowhere.
fn tile(d: &Dashboard) -> AnyView {
    let (href, note) = if d.frontend_running {
        (dashboards::open_url(d), None)
    } else {
        (
            dashboards::wake_url(d),
            Some("Not running — opening it starts it"),
        )
    };
    let face = view! {
        <span class="adi-new-app__icon">
            <Icon icon=Lucide::LayoutDashboard size=IconSize::Xl/>
        </span>
        <span class="adi-new-app__name">{d.name.clone()}</span>
    };
    match href {
        // A new tab, as the old rail opens one: an app is somewhere you keep open beside this.
        Some(href) => view! {
            <a
                class="adi-new-app"
                href=href
                target="_blank"
                rel="noopener"
                title=note.map_or_else(|| d.name.clone(), |n| format!("{} — {n}", d.name))
            >
                {face}
            </a>
        }
        .into_any(),
        None => view! {
            <span
                class="adi-new-app is-off"
                title=format!("{} — not running, and no address to start it at", d.name)
            >
                {face}
            </span>
        }
        .into_any(),
    }
}
