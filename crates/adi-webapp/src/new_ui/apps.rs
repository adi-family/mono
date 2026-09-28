//! The apps on the wallpaper, as iOS lays out a home screen: a tile for each app, its name
//! beneath, in rows from the top — this machine's first, then each paired machine's under its
//! name.
//!
//! What a tile does follows the old chat's Apps rail (`chat_dash_item`, `chat_node_dash_item`),
//! through the same helpers — except that an app opens in the app window ([`super::browser`])
//! rather than a tab:
//!
//! * here, a running dashboard opens where it answers ([`dashboards::open_url`]); a stopped one
//!   with a routable host is still a link, to the address that wakes it ([`dashboards::wake_url`]),
//!   because dashboards stop when idle and start on the first request;
//! * on a paired machine, one this machine has been granted opens at `<app>.<machine>.n.adi`,
//!   running or not — that address wakes it the same way over there; one running but not granted
//!   asks the machine for the grant when pressed, since pairing grants only its control panel;
//! * anything with nowhere to go stays as a dimmed tile, so an app is never silently missing.
//!
//! A paired machine that is locked (no password held here) or refused is left out: there is
//! nothing of its to draw, and the top bar's device list is where a machine's state is read.
//!
//! A tile's picture is the app's own `frontend/favicon.svg`, read as a file from its directory
//! ([`Apps::pictures`]) rather than fetched from the app — so a stopped app shows it too.

use std::collections::HashMap;

use adi_ui::{Icon, IconSize, Lucide};
use adi_webapp_api::types::{Dashboard, FleetDashboards, NodeDashboard, NodeDashboards};
use leptos::{ev, prelude::*};

use super::windows::{AppRef, Desk};
use crate::pages::dashboards;
use crate::{fetch, origin};

/// How often the apps are read again. A dashboard started or stopped elsewhere shows up within
/// this; the paired machines' half is a mesh round trip per machine, so not more often.
const TICK_MS: u32 = 30_000;

#[derive(Clone, Copy)]
pub(super) struct Apps {
    /// This machine's dashboards, archived ones left out. `None` until the first answer.
    local: RwSignal<Option<Vec<Dashboard>>>,
    /// Every paired machine's listing, as `/api/fleet/dashboards` answers it.
    remote: RwSignal<Option<Vec<NodeDashboards>>>,
    /// The tile whose grant is being asked for, by [`Tile::key`].
    asking: RwSignal<Option<String>>,
    /// The last ask that failed: which tile, and the node's refusal.
    refused: RwSignal<Option<(String, String)>>,
    /// Each app's `frontend/favicon.svg` as a `data:` URL, by [`Tile::key`]; `None` once asked
    /// and found missing (or still on its way). Asked once per tile for the life of the screen.
    pictures: RwSignal<HashMap<String, Option<String>>>,
}

impl Apps {
    /// Read now, and again every [`TICK_MS`] for as long as the calling scope lives.
    pub(super) fn load() -> Self {
        let apps = Self {
            local: RwSignal::new(None),
            remote: RwSignal::new(None),
            asking: RwSignal::new(None),
            refused: RwSignal::new(None),
            pictures: RwSignal::new(HashMap::new()),
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

    // Two reads, not one after the other: the local list is instant and the paired machines'
    // waits on every dial, so this machine's apps are never held back by a sleeping laptop.
    // A failed read keeps the last answer either way: stale by a tick beats an emptied screen.
    fn read(self) {
        leptos::task::spawn_local(async move {
            if let Ok(state) = fetch::dashboards().await {
                let live = state
                    .dashboards
                    .into_iter()
                    .filter(|d| !d.is_archived())
                    .collect::<Vec<Dashboard>>();
                for d in &live {
                    self.picture(format!(":{}", d.id), None, &d.id);
                }
                self.local.set(Some(live));
            }
        });
        leptos::task::spawn_local(async move {
            if let Ok(f) = fetch::fleet_dashboards().await {
                for n in f.nodes.iter().filter(|n| !n.locked && n.error.is_none()) {
                    for d in &n.dashboards {
                        self.picture(format!("{}:{}", n.node, d.id), Some(&n.node), &d.id);
                    }
                }
                self.remote.set(Some(f.nodes));
            }
        });
    }

    /// Read app `id`'s picture off `node` (this machine for `None`) into [`Self::pictures`], unless
    /// it has been asked for already.
    ///
    /// Through the panel's store browser, `/api/fs/read` — on a paired machine via its own panel,
    /// `/api/node/<node>/…` — because the file is text and that route already exists on every
    /// panel, released or not. An SVG in an `<img>` runs no script, so a machine's picture cannot
    /// act on this page.
    fn picture(self, key: String, node: Option<&str>, id: &str) {
        if self.pictures.with_untracked(|p| p.contains_key(&key)) {
            return;
        }
        self.pictures.update(|p| {
            p.insert(key.clone(), None);
        });
        let node = node.map(str::to_string);
        let path = format!("dashboards/{id}/frontend/favicon.svg");
        leptos::task::spawn_local(async move {
            if let Ok(file) = fetch::fs_read_on(node.as_deref(), &path).await {
                let url = format!(
                    "data:image/svg+xml;charset=utf-8,{}",
                    js_sys::encode_uri_component(&file.content)
                );
                self.pictures.update(|p| {
                    p.insert(key, Some(url));
                });
            }
        });
    }

    /// Ask `node` to let this machine open `service`. The answer is the whole listing again, so
    /// the tile turns into a link in the same round trip.
    fn ask(self, key: String, node: String, service: String) {
        self.asking.set(Some(key.clone()));
        self.refused.set(None);
        leptos::task::spawn_local(async move {
            match fetch::allow_node_service(node, service).await {
                Ok(FleetDashboards { nodes }) => self.remote.set(Some(nodes)),
                Err(e) => self.refused.set(Some((key, e))),
            }
            self.asking.set(None);
        });
    }

    /// The home screen as sections: this machine's apps, then one per paired machine that listed
    /// any. Tracked.
    fn sections(self) -> Vec<(Option<String>, Vec<Tile>)> {
        let mut out = Vec::new();
        let pictures = self.pictures.get();
        let here: Vec<Tile> = self.local.with(|l| {
            l.iter()
                .flatten()
                .map(|d| Tile::local(d).with_picture(&pictures))
                .collect()
        });
        if !here.is_empty() {
            out.push((None, here));
        }
        self.remote.with(|r| {
            for n in r.iter().flatten() {
                if n.locked || n.error.is_some() || n.dashboards.is_empty() {
                    continue;
                }
                let tiles = n
                    .dashboards
                    .iter()
                    .map(|d| Tile::remote(&n.node, d).with_picture(&pictures))
                    .collect();
                out.push((Some(n.node.clone()), tiles));
            }
        });
        out
    }
}

/// What one tile is, worked out before anything is drawn — and compared, so a tick that changes
/// nothing redraws nothing.
#[derive(Clone, PartialEq)]
struct Tile {
    /// Unique on the screen: the machine (empty for this one) and the app's id.
    key: String,
    id: String,
    name: String,
    /// The paired machine it runs on; `None` for this one.
    machine: Option<String>,
    /// Its picture: the app's own `favicon.svg` read from its directory; failing that, where its
    /// favicon is while it runs (see [`favicon`]).
    favicon: Option<String>,
    /// The Lucide icon it names as its picture, if any (see [`AppMark`]).
    icon: Option<String>,
    /// Where pressing it goes, when it goes anywhere.
    href: Option<String>,
    /// The grant to ask for when it is pressed instead: the machine and the service.
    ask: Option<(String, String)>,
    /// What hovering it says.
    note: String,
}

impl Tile {
    /// Prefer the picture read from the app's directory to the favicon address worked out from
    /// its link: it is there whether or not the app runs.
    fn with_picture(mut self, pictures: &HashMap<String, Option<String>>) -> Self {
        if let Some(Some(url)) = pictures.get(&self.key) {
            self.favicon = Some(url.clone());
        }
        self
    }

    fn local(d: &Dashboard) -> Self {
        let (href, note) = if d.frontend_running {
            (dashboards::open_url(d), d.name.clone())
        } else {
            (
                dashboards::wake_url(d),
                format!("{} — not running; opening it starts it", d.name),
            )
        };
        let note = if href.is_some() {
            note
        } else {
            format!("{} — not running, and no address to start it at", d.name)
        };
        let favicon = if d.frontend_running {
            href.as_deref().and_then(favicon)
        } else {
            None
        };
        Self {
            favicon,
            icon: d.icon.clone(),
            key: format!(":{}", d.id),
            id: d.id.clone(),
            name: d.name.clone(),
            machine: None,
            href,
            ask: None,
            note,
        }
    }

    fn remote(node: &str, d: &NodeDashboard) -> Self {
        let mapped = d.url.as_deref().and_then(origin::mapped_url);
        let (href, ask, note) = match (d.allowed, mapped, d.service.clone(), d.running) {
            (true, Some(href), _, true) => (Some(href), None, format!("{} on {node}", d.name)),
            (true, Some(href), _, false) => (
                Some(href),
                None,
                format!("{} on {node} — not running; opening it starts it", d.name),
            ),
            (false, _, Some(service), true) => (
                None,
                Some((node.to_string(), service.clone())),
                format!(
                    "{} on {node} — {node} has not let this machine open it. Press to ask.",
                    d.name
                ),
            ),
            (_, _, _, false) => (None, None, format!("{} — not running on {node}", d.name)),
            _ => (
                None,
                None,
                format!("{} on {node} — no address reaches it from here", d.name),
            ),
        };
        let favicon = if d.running && d.allowed {
            href.as_deref().and_then(favicon)
        } else {
            None
        };
        Self {
            favicon,
            icon: d.icon.clone(),
            key: format!("{node}:{}", d.id),
            id: d.id.clone(),
            name: d.name.clone(),
            machine: Some(node.to_string()),
            href,
            ask,
            note,
        }
    }
}

/// The grid. Draws nothing until there is an app, so a machine with none keeps a bare wallpaper.
#[component]
pub(super) fn Home(apps: Apps, desk: Desk, #[prop(into)] light: Signal<bool>) -> impl IntoView {
    let sections = Memo::new(move |_| apps.sections());
    view! {
        <Show when=move || sections.with(|s| !s.is_empty())>
            <nav class="adi-new-apps" class:light=move || light.get() aria-label="Apps">
                {move || {
                    sections
                        .get()
                        .into_iter()
                        .map(|(machine, tiles)| {
                            view! {
                                {machine.map(|m| view! {
                                    <h2 class="adi-new-apps__machine">{m}</h2>
                                })}
                                {tiles.into_iter().map(|t| tile(apps, desk, t)).collect_view()}
                            }
                        })
                        .collect_view()
                }}
            </nav>
        </Show>
    }
}

/// A tile's face: `mark` on the tile, the name beneath.
fn face(mark: impl IntoView + 'static, name: impl IntoView + 'static) -> impl IntoView {
    view! {
        <span class="adi-new-app__icon">{mark}</span>
        <span class="adi-new-app__name">{name}</span>
    }
}

/// Where an app's favicon would be: `/favicon.ico` on its own origin, where browsers have always
/// looked first.
///
/// Asked for only while the app runs. A stopped one's address is also the address that starts it,
/// so a picture of it would start every app on the screen just by drawing it.
fn favicon(href: &str) -> Option<String> {
    let (scheme, rest) = href.split_once("://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    Some(format!("{scheme}://{host}/favicon.ico"))
}

/// An app's mark: its favicon when it has one that loads; else the Lucide icon its `config.toml`
/// names; else the first letter of its name — on a home-screen tile and on the island alike.
///
/// An icon name this build's set lacks falls through to the letter: the name comes from the app,
/// possibly from a machine running a newer panel, and a blank tile would be worse than a letter.
#[component]
pub(super) fn AppMark(
    name: String,
    favicon: Option<String>,
    icon: Option<String>,
) -> impl IntoView {
    let glyph = icon.as_deref().and_then(Lucide::from_name);
    let letter = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map_or_else(|| "·".to_string(), |c| c.to_uppercase().to_string());
    // Anything that is not an image — the front door's page for a name it does not know, an app
    // that serves no icon — fails to load as one, and the glyph or the letter stands in.
    let failed = RwSignal::new(favicon.is_none());
    view! {
        <Show
            when=move || !failed.get()
            fallback=move || match glyph {
                Some(g) => view! { <Icon icon=g size=IconSize::Xl/> }.into_any(),
                None => view! {
                    <span class="adi-new-mark__letter">{letter.clone()}</span>
                }
                .into_any(),
            }
        >
            <img
                class="adi-new-mark__img"
                src=favicon.clone()
                alt=""
                on:error=move |_| failed.set(true)
            />
        </Show>
    }
}

fn tile(apps: Apps, desk: Desk, t: Tile) -> AnyView {
    let Tile {
        key,
        id,
        name,
        machine,
        favicon,
        icon,
        href,
        ask,
        note,
    } = t;
    if let Some(href) = href {
        let app = AppRef {
            key: machine
                .as_ref()
                .map_or_else(|| id.clone(), |m| format!("{m}/{id}")),
            name: name.clone(),
            machine,
            url: href.clone(),
            favicon: favicon.clone(),
            icon: icon.clone(),
        };
        // Opens in the app window. Still a real link to the app underneath, so a modified or
        // middle click does what it does on any link — a tab of its own — and is left alone.
        return view! {
            <a
                class="adi-new-app"
                href=href
                target="_blank"
                rel="noopener"
                title=note
                on:click=move |ev: ev::MouseEvent| {
                    if ev.button() == 0
                        && !(ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.alt_key())
                    {
                        ev.prevent_default();
                        desk.open_app(app.clone());
                    }
                }
            >
                {face(view! { <AppMark name=name.clone() favicon icon/> }, name)}
            </a>
        }
        .into_any();
    }
    let Some((node, service)) = ask else {
        return view! {
            <span class="adi-new-app is-off" title=note>
                {face(view! { <AppMark name=name.clone() favicon icon/> }, name)}
            </span>
        }
        .into_any();
    };
    let (k1, k2) = (key.clone(), key.clone());
    let busy = Signal::derive(move || apps.asking.with(|a| a.as_deref() == Some(k1.as_str())));
    let failed = Signal::derive(move || {
        apps.refused
            .with(|r| r.as_ref().filter(|(k, _)| *k == k2).map(|(_, e)| e.clone()))
    });
    let label = move || {
        if busy.get() {
            "Asking…".to_string()
        } else if failed.with(Option::is_some) {
            "Refused".to_string()
        } else {
            name.clone()
        }
    };
    view! {
        <button
            class="adi-new-app is-ask"
            class:is-refused=move || failed.with(Option::is_some)
            type="button"
            title=move || failed.get().unwrap_or_else(|| note.clone())
            prop:disabled=busy
            on:click=move |_| apps.ask(key.clone(), node.clone(), service.clone())
        >
            {face(view! { <Icon icon=Lucide::Lock size=IconSize::Xl/> }, label)}
        </button>
    }
    .into_any()
}
