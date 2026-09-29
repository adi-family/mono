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
//!
//! The listings and the pictures start from the last load's ([`super::cache`]): a reload draws
//! the whole home screen at once, and the reads only update it.
//!
//! An app whose `config.toml` declares widgets also gets them: in the grid ahead of its machine's
//! tiles, or — one that asks for `half` — across the screen's right half, beside the grid
//! ([`super::widgets`]).
//!
//! The order of all of it, each widget's size and what is left off are the person's to change
//! ([`super::arrange`]), in the home screen's editing mode.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use adi_ui::{Icon, IconSize, Lucide, Menu, MenuAt, MenuItem};
use adi_webapp_api::types::{Dashboard, FleetDashboards, NodeDashboard, NodeDashboards};
use leptos::{ev, portal::Portal, prelude::*};

use super::arrange::{self, Arrange, Arrangement};
use super::cache;
use super::widgets::{self, AppWidget, Declared, Size};
use super::windows::{AppRef, Desk};
use crate::pages::dashboards;
use crate::{fetch, origin};

/// How often the apps are read again. A dashboard started or stopped elsewhere shows up within
/// this; the paired machines' half is a mesh round trip per machine, so not more often.
const TICK_MS: u32 = 30_000;

const LOCAL_KEY: &str = "apps-local";
const REMOTE_KEY: &str = "apps-remote";
const PICTURES_KEY: &str = "apps-pictures";
// Not `apps-widgets`: that held the sizes read out of `frontend/widget.html`, which no longer
// means anything.
const WIDGETS_KEY: &str = "apps-widgets-declared";

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
    /// Each app's `frontend/favicon.svg` as a `data:` URL, by [`Tile::key`], as last read.
    pictures: RwSignal<HashMap<String, String>>,
    /// The widgets each app's `config.toml` declares, by [`Tile::key`]; absent until read.
    widgets: RwSignal<HashMap<String, Vec<Declared>>>,
    /// The files asked for since this screen mounted, by kind and [`Tile::key`]: once each, so a
    /// cached answer is still checked once per load, and never once per tick.
    asked: StoredValue<HashSet<String>>,
}

impl Apps {
    /// Read now, and again every [`TICK_MS`] for as long as the calling scope lives.
    pub(super) fn load() -> Self {
        let apps = Self {
            local: RwSignal::new(cache::load(LOCAL_KEY)),
            remote: RwSignal::new(cache::load(REMOTE_KEY)),
            asking: RwSignal::new(None),
            refused: RwSignal::new(None),
            pictures: RwSignal::new(cache::load(PICTURES_KEY).unwrap_or_default()),
            widgets: RwSignal::new(cache::load(WIDGETS_KEY).unwrap_or_default()),
            asked: StoredValue::new(HashSet::new()),
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
                    self.widget(format!(":{}", d.id), None, &d.id);
                }
                cache::save(LOCAL_KEY, &live);
                self.local.set(Some(live));
            }
        });
        leptos::task::spawn_local(async move {
            if let Ok(f) = fetch::fleet_dashboards().await {
                for n in f.nodes.iter().filter(|n| !n.locked && n.error.is_none()) {
                    for d in &n.dashboards {
                        self.picture(format!("{}:{}", n.node, d.id), Some(&n.node), &d.id);
                        self.widget(format!("{}:{}", n.node, d.id), Some(&n.node), &d.id);
                    }
                }
                cache::save(REMOTE_KEY, &f.nodes);
                self.remote.set(Some(f.nodes));
            }
        });
    }

    /// Read app `id`'s picture off `node` (this machine for `None`) into [`Self::pictures`], unless
    /// it has been asked for already since the screen mounted. A picture that fails to read keeps
    /// the one cached, if any: an unreachable machine is not a reason to blank its tiles.
    ///
    /// Through the panel's store browser, `/api/fs/read` — on a paired machine via its own panel,
    /// `/api/node/<node>/…` — because the file is text and that route already exists on every
    /// panel, released or not. An SVG in an `<img>` runs no script, so a machine's picture cannot
    /// act on this page.
    fn picture(self, key: String, node: Option<&str>, id: &str) {
        if !self.first_ask(format!("picture|{key}")) {
            return;
        }
        let node = node.map(str::to_string);
        let path = format!("dashboards/{id}/frontend/favicon.svg");
        leptos::task::spawn_local(async move {
            if let Ok(file) = fetch::fs_read_on(node.as_deref(), &path).await {
                let url = format!(
                    "data:image/svg+xml;charset=utf-8,{}",
                    js_sys::encode_uri_component(&file.content)
                );
                // Only a changed picture is written, so re-checking the cached ones redraws nothing.
                if self.pictures.with_untracked(|p| p.get(&key) != Some(&url)) {
                    self.pictures.update(|p| {
                        p.insert(key, url);
                    });
                    self.pictures.with_untracked(|p| cache::save(PICTURES_KEY, p));
                }
            }
        });
    }

    /// Whether `what` is being asked for the first time since the screen mounted — and, if so,
    /// mark it asked.
    fn first_ask(self, what: String) -> bool {
        self.asked
            .try_update_value(|a| a.insert(what))
            .unwrap_or(false)
    }

    /// Read the widgets app `id` on `node` declares in its `config.toml` into [`Self::widgets`] —
    /// read as a file, as its picture is ([`Self::picture`]).
    ///
    /// Unlike a picture, widgets the machine says are gone are dropped from the cache: a frame
    /// kept for a widget nobody declares any more would show the app's 404 on the home screen. A
    /// read that fails for any other reason — the machine not answering — keeps what was cached.
    fn widget(self, key: String, node: Option<&str>, id: &str) {
        if !self.first_ask(format!("widget|{key}")) {
            return;
        }
        let node = node.map(str::to_string);
        let path = format!("dashboards/{id}/config.toml");
        leptos::task::spawn_local(async move {
            let now = match fetch::fs_read_on(node.as_deref(), &path).await {
                Ok(file) => widgets::declared(&file.content),
                Err(e) if e.contains("no such file") => Vec::new(),
                Err(_) => return,
            };
            if self.widgets.with_untracked(|w| w.get(&key) != Some(&now)) {
                self.widgets.update(|w| {
                    w.insert(key, now);
                });
                self.widgets.with_untracked(|w| cache::save(WIDGETS_KEY, w));
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
        let widgets = self.widgets.get();
        let here: Vec<Tile> = self.local.with(|l| {
            l.iter()
                .flatten()
                .map(|d| Tile::local(d).with_files(&pictures, &widgets))
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
                    .map(|d| Tile::remote(&n.node, d).with_files(&pictures, &widgets))
                    .collect();
                out.push((Some(n.node.clone()), tiles));
            }
        });
        out
    }
}

/// What one tile is, worked out before anything is drawn — and compared, so a tick that changes
/// nothing redraws nothing.
#[derive(Clone, PartialEq, Eq, Hash)]
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
    /// The widgets it declares.
    widgets: Vec<Declared>,
    /// Where pressing it goes, when it goes anywhere.
    href: Option<String>,
    /// The grant to ask for when it is pressed instead: the machine and the service.
    ask: Option<(String, String)>,
    /// What hovering it says.
    note: String,
}

impl Tile {
    /// What was read from the app's directory: its picture — preferred to the favicon address
    /// worked out from its link, being there whether or not the app runs — and its widgets.
    fn with_files(
        mut self,
        pictures: &HashMap<String, String>,
        widgets: &HashMap<String, Vec<Declared>>,
    ) -> Self {
        if let Some(url) = pictures.get(&self.key) {
            self.favicon = Some(url.clone());
        }
        self.widgets = widgets.get(&self.key).cloned().unwrap_or_default();
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
            widgets: Vec::new(),
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
            widgets: Vec::new(),
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

/// One thing in the grid, in order: a machine's heading, an app's widget, an app's tile.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Item {
    Machine(String),
    Widget {
        id: String,
        name: String,
        url: String,
        size: Size,
    },
    Tile(Tile),
}

impl Item {
    /// The item's identity for [`For`]: everything it draws. An item that did not change keeps its
    /// node — which for a widget is the difference between a live frame and one that reloads its
    /// page on every read.
    fn key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// A tile's widgets, as far as there is somewhere to frame them from: path, name, address and
/// declared size.
fn framed(t: &Tile) -> Vec<(String, String, String, Size)> {
    let Some(href) = t.href.as_deref() else {
        return Vec::new();
    };
    t.widgets
        .iter()
        .filter_map(|w| {
            let url = widgets::url(href, &w.path)?;
            Some((
                w.path.clone(),
                w.name.clone().unwrap_or_else(|| t.name.clone()),
                url,
                w.size,
            ))
        })
        .collect()
}

/// The widget beside the grid: its id, name and address.
#[derive(Clone, PartialEq)]
struct Half {
    id: String,
    name: String,
    url: String,
}

/// The home screen as arranged.
#[derive(Clone, PartialEq)]
struct Laid {
    /// The widget that takes the right half: one chosen for it while editing, else the first,
    /// this machine's before a paired one's, whose app asks for [`Size::Half`]. Any other that
    /// asks is drawn large, in the grid.
    half: Option<Half>,
    /// The grid as a flat list: each section's heading, then its widgets and tiles as arranged,
    /// the half left out.
    items: Vec<Item>,
    /// Every item shown, the half's included, in order — what a drag reorders.
    ids: Vec<String>,
}

/// The listing, put in the order and sizes `arr` holds, with what it hides left out. Tracked.
fn laid(apps: Apps, arr: &Arrangement) -> Laid {
    let mut sections = Vec::new();
    for (machine, tiles) in apps.sections() {
        let mut entries = Vec::new();
        for t in &tiles {
            for (path, name, url, size) in framed(t) {
                let id = arrange::widget_id(&t.key, &path);
                let size = arr.size(&id, size);
                entries.push((id.clone(), Item::Widget { id, name, url, size }));
            }
        }
        entries.extend(tiles.into_iter().map(|t| (arrange::app_id(&t.key), Item::Tile(t))));
        entries.retain(|(id, _)| !arr.is_hidden(id));
        sections.push((machine, arr.sorted(entries)));
    }
    let shown = || sections.iter().flat_map(|(_, e)| e.iter());
    let half = arr
        .chosen_half()
        .and_then(|h| shown().find(|(id, _)| id == h))
        .or_else(|| {
            shown().find(|(_, i)| matches!(i, Item::Widget { size: Size::Half, .. }))
        })
        .and_then(|(_, i)| match i {
            Item::Widget { id, name, url, .. } => Some(Half {
                id: id.clone(),
                name: name.clone(),
                url: url.clone(),
            }),
            _ => None,
        });
    let ids = shown().map(|(id, _)| id.clone()).collect();
    let beside = half.as_ref().map(|h| h.id.as_str());
    let mut items = Vec::new();
    for (machine, entries) in sections {
        if entries.is_empty() {
            continue;
        }
        items.extend(machine.map(Item::Machine));
        items.extend(
            entries
                .into_iter()
                .filter(|(id, _)| Some(id.as_str()) != beside)
                .map(|(_, i)| i),
        );
    }
    Laid { half, items, ids }
}

/// What the home screen's own menus were opened on.
#[derive(Clone)]
enum Menued {
    /// The wallpaper, between the tiles.
    Screen,
    /// An app's tile: the app, and its item id.
    Tile(AppRef, String),
    /// A widget's size, while editing: its id and the size it is drawn at.
    Size(String, Size),
}

/// The grid. Draws nothing until there is an app, so a machine with none keeps a bare wallpaper.
///
/// Editing (see [`super::arrange`]) is the home screen's own mode, as iOS's: every item can be
/// dragged among its machine's, a widget resized, anything taken off; tiles stop opening. Entered
/// from a right-click on the wallpaper or a tile, or the palette; left with Done, `Escape`, or a
/// click on the wallpaper.
#[component]
pub(super) fn Home(
    apps: Apps,
    desk: Desk,
    home: Arrange,
    #[prop(into)] light: Signal<bool>,
) -> impl IntoView {
    let laid = Memo::new(move |_| home.now.with(|arr| laid(apps, arr)));
    let items = Memo::new(move |_| laid.with(|l| l.items.clone()));
    // A memo, so a read that leaves it as it was does not rebuild the frame.
    let half = Memo::new(move |_| laid.with(|l| l.half.clone()));
    // One menu for the whole screen — what it was opened on, and where.
    let menu: RwSignal<Option<(MenuAt, Menued)>> = RwSignal::new(None);
    let editing = home.editing;
    let on_screen = move |ev: ev::MouseEvent| {
        // A tile's own menu has already answered it.
        if ev.default_prevented() {
            return;
        }
        ev.prevent_default();
        menu.set(Some((MenuAt::Point(ev.client_x(), ev.client_y()), Menued::Screen)));
    };
    // A click on the wallpaper itself, not on anything drawn on it, ends editing.
    let off_screen = move |ev: ev::MouseEvent| {
        if editing.get_untracked() && ev.target() == ev.current_target() {
            home.edit(false);
        }
    };
    view! {
        <div
            class="adi-new-home"
            class:has-half=move || half.with(Option::is_some)
            class:is-editing=move || editing.get()
            on:contextmenu=on_screen
            on:click=off_screen
        >
        <Show when=move || editing.get()>
            <EditBar home light/>
        </Show>
        <Show when=move || items.with(|i| !i.is_empty())>
            <nav
                class="adi-new-apps"
                class:light=move || light.get()
                aria-label="Apps"
                on:click=off_screen
            >
                <For each=move || items.get() key=Item::key let:item>
                    {match item {
                        Item::Machine(m) => view! {
                            <h2 class="adi-new-apps__machine">{m}</h2>
                        }
                        .into_any(),
                        Item::Widget { id, name, url, size } => {
                            // Only one widget has the half; any other that asks for it is large.
                            let size = if size == Size::Half { Size::Large } else { size };
                            let body = view! { <AppWidget name url size light/> }.into_any();
                            slot(home, laid, menu, id, Some(size), true, body)
                        }
                        Item::Tile(t) => {
                            let id = arrange::app_id(&t.key);
                            let body = tile(apps, desk, home, menu, id.clone(), t);
                            slot(home, laid, menu, id, None, true, body)
                        }
                    }}
                </For>
            </nav>
        </Show>
        {move || half.get().map(|Half { id, name, url }| {
            let body = view! { <AppWidget name url size=Size::Half light/> }.into_any();
            slot(home, laid, menu, id, Some(Size::Half), false, body)
        })}
        {move || menu.get().map(|(at, on)| {
            screen_menu(desk, home, laid, menu, at, on, light.get_untracked())
        })}
        </div>
    }
}

/// The strip over the grid while editing: what to do, and the ways out of it.
#[component]
fn EditBar(home: Arrange, #[prop(into)] light: Signal<bool>) -> impl IntoView {
    let hidden = move || home.now.with(Arrangement::hidden_count);
    view! {
        <div class="adi-new-edit" class:light=move || light.get() role="toolbar" aria-label="Edit home screen">
            <span class="adi-new-edit__hint">"Drag to rearrange"</span>
            <Show when=move || { hidden() > 0 }>
                <button
                    class="adi-btn adi-btn--quiet adi-btn--sm"
                    type="button"
                    on:click=move |_| home.show_hidden()
                >
                    {move || format!("Show {} hidden", hidden())}
                </button>
            </Show>
            <button
                class="adi-btn adi-btn--quiet adi-btn--sm"
                type="button"
                title="Back to the listing's order, every widget at its app's size, nothing hidden"
                prop:disabled=move || home.now.with(Arrangement::is_empty)
                on:click=move |_| home.reset()
            >
                "Reset"
            </button>
            <button
                class="adi-btn adi-btn--strong adi-btn--sm"
                type="button"
                on:click=move |_| home.edit(false)
            >
                "Done"
            </button>
        </div>
    }
}

/// One item's place on the home screen: what drags it and what it is dropped on, and — while
/// editing — its remove button and, for a widget, its size.
///
/// A widget's page is covered while editing: a frame takes the pointer, so a drag that began over
/// it would never reach this page.
fn slot(
    home: Arrange,
    laid: Memo<Laid>,
    menu: RwSignal<Option<(MenuAt, Menued)>>,
    id: String,
    widget: Option<Size>,
    movable: bool,
    body: AnyView,
) -> AnyView {
    let class = match widget {
        None => "adi-new-slot is-app",
        Some(Size::Small) => "adi-new-slot is-small",
        Some(Size::Medium) => "adi-new-slot is-medium",
        Some(Size::Large) => "adi-new-slot is-large",
        Some(Size::Half) => "adi-new-slot is-half",
    };
    let editing = home.editing;
    let id = StoredValue::new(id);
    let is = move |s: RwSignal<Option<String>>| {
        s.with(|d| id.with_value(|id| d.as_deref() == Some(id.as_str())))
    };
    let start = move |ev: ev::DragEvent| {
        if !movable || !editing.get_untracked() {
            ev.prevent_default();
            return;
        }
        if let Some(dt) = ev.data_transfer() {
            // Firefox starts no drag that carries no data.
            let _ = dt.set_data("text/plain", &id.get_value());
            dt.set_effect_allowed("move");
        }
        home.dragging.set(Some(id.get_value()));
    };
    let over = move |ev: ev::DragEvent| {
        let Some(from) = home.dragging.get_untracked() else {
            return;
        };
        if !movable || !id.with_value(|id| arrange::same_machine(&from, id)) {
            return;
        }
        ev.prevent_default();
        if !is(home.target) {
            home.target.set(Some(id.get_value()));
        }
    };
    let drop = move |ev: ev::DragEvent| {
        ev.prevent_default();
        if let Some(from) = home.dragging.get_untracked() {
            laid.with_untracked(|l| id.with_value(|to| home.put(&l.ids, &from, to)));
        }
        home.dragging.set(None);
        home.target.set(None);
    };
    let end = move |_: ev::DragEvent| {
        home.dragging.set(None);
        home.target.set(None);
    };
    let remove = move |ev: ev::MouseEvent| {
        ev.stop_propagation();
        id.with_value(|id| home.hide(id));
    };
    let resize = move |ev: ev::MouseEvent| {
        ev.stop_propagation();
        if let Some(size) = widget {
            menu.set(Some((
                MenuAt::Point(ev.client_x(), ev.client_y()),
                Menued::Size(id.get_value(), size),
            )));
        }
    };
    view! {
        <div
            class=class
            class:is-dragged=move || is(home.dragging)
            class:is-target=move || is(home.target) && !is(home.dragging)
            draggable=move || if movable && editing.get() { "true" } else { "false" }
            on:dragstart=start
            on:dragover=over
            on:drop=drop
            on:dragend=end
        >
            {body}
            <Show when=move || editing.get()>
                {widget.map(|size| view! {
                    <div class="adi-new-slot__cover">
                        <button
                            class="adi-btn adi-btn--sm adi-new-slot__size"
                            type="button"
                            title="Widget size"
                            on:click=resize
                        >
                            {size.label()}
                            <Icon icon=Lucide::ChevronDown size=IconSize::Sm/>
                        </button>
                    </div>
                })}
                <button
                    class="adi-new-slot__remove"
                    type="button"
                    title="Remove from home screen"
                    aria-label="Remove from home screen"
                    on:click=remove
                >
                    <Icon icon=Lucide::Minus size=IconSize::Sm/>
                </button>
            </Show>
        </div>
    }
    .into_any()
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
                draggable="false"
                on:error=move |_| failed.set(true)
            />
        </Show>
    }
}

/// The home screen's right-click menu, and a widget's size menu while editing.
///
/// * on a tile: open the app — in its window, or in a new one beside it — then edit the home
///   screen, or take the tile off it;
/// * on the wallpaper: edit the home screen;
/// * on a widget's size button: the four sizes, the one it has ticked.
///
/// Out of the grid into the body, as the devices drop-down is: the home screen sits under blurred
/// surfaces, and a fixed menu inside one of them is placed against it rather than the screen.
fn screen_menu(
    desk: Desk,
    home: Arrange,
    laid: Memo<Laid>,
    menu: RwSignal<Option<(MenuAt, Menued)>>,
    at: MenuAt,
    on: Menued,
    light: bool,
) -> AnyView {
    // Built outside the portal: its children are drawn by a closure that may run more than once,
    // so what they capture has to be `Copy`, and a callback is.
    let act = move |f: Box<dyn Fn() + Send + Sync>| {
        Callback::new(move |()| {
            menu.set(None);
            f();
        })
    };
    // Each row: its words, what it does, and — in the size menu — whether it is the one picked.
    let mut rows: Vec<(&'static str, Callback<()>, Option<bool>)> = Vec::new();
    let edit = (!home.editing.get_untracked())
        .then(|| ("Edit home screen", act(Box::new(move || home.edit(true))), None));
    match on {
        Menued::Screen => rows.extend(edit),
        Menued::Tile(app, id) => {
            let again = app.clone();
            rows.push(("Open", act(Box::new(move || desk.open_app(app.clone()))), None));
            rows.push((
                "Open in new window",
                act(Box::new(move || desk.open_app_new(again.clone()))),
                None,
            ));
            rows.extend(edit);
            rows.push(("Remove from home screen", act(Box::new(move || home.hide(&id))), None));
        }
        Menued::Size(id, now) => rows.extend(Size::ALL.into_iter().map(|size| {
            let id = id.clone();
            let pick = act(Box::new(move || {
                let half = laid.with_untracked(|l| l.half.as_ref().map(|h| h.id.clone()));
                home.resize(&id, size, half.as_deref());
            }));
            (size.label(), pick, Some(size == now))
        })),
    }
    let rows = StoredValue::new(rows);
    let class = if light { "light" } else { "" };
    view! {
        <Portal>
            <Menu
                at=Signal::derive(move || Some(at))
                on_dismiss=Callback::new(move |()| menu.set(None))
                class=class
            >
                {rows
                    .get_value()
                    .into_iter()
                    .map(|(label, on, checked)| match checked {
                        Some(c) => view! {
                            <MenuItem on_select=on checked=c radio=true>{label}</MenuItem>
                        }
                        .into_any(),
                        None => view! { <MenuItem on_select=on>{label}</MenuItem> }.into_any(),
                    })
                    .collect_view()}
            </Menu>
        </Portal>
    }
    .into_any()
}

fn tile(
    apps: Apps,
    desk: Desk,
    home: Arrange,
    menu: RwSignal<Option<(MenuAt, Menued)>>,
    item: String,
    t: Tile,
) -> AnyView {
    let editing = home.editing;
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
        widgets: _,
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
        let for_menu = app.clone();
        return view! {
            <a
                class="adi-new-app"
                href=href
                target="_blank"
                rel="noopener"
                title=note
                // A link drags its address; while editing, the drag is the tile's slot.
                draggable=move || if editing.get() { "false" } else { "true" }
                on:contextmenu=move |ev: ev::MouseEvent| {
                    ev.prevent_default();
                    menu.set(Some((
                        MenuAt::Point(ev.client_x(), ev.client_y()),
                        Menued::Tile(for_menu.clone(), item.clone()),
                    )));
                }
                on:click=move |ev: ev::MouseEvent| {
                    if editing.get_untracked() {
                        ev.prevent_default();
                    } else if ev.button() == 0
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
            on:click=move |_| {
                if !editing.get_untracked() {
                    apps.ask(key.clone(), node.clone(), service.clone());
                }
            }
        >
            {face(view! { <Icon icon=Lucide::Lock size=IconSize::Xl/> }, label)}
        </button>
    }
    .into_any()
}
