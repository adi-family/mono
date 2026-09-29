//! The new UI's windows, and the desk they sit on.
//!
//! Every window has an address. Visiting it opens that window — or brings it to the front — and
//! leaves every other window open where it was, the way a second app on macOS leaves the first
//! one's windows alone. The address bar names the front window; closing a window closes that one
//! and nothing else, and the address moves to whichever window is in front after it (`/` once the
//! desk is clear).
//!
//! Which windows are open, in which order, and where each was dropped, are remembered per device
//! in `localStorage` — the same place as the wallpaper, for the same reason.

use leptos::{ev, html, prelude::*};
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;

use crate::{routing, ui};

/// The open windows, back to front.
const OPEN_KEY: &str = "adi-new-ui-windows";

/// Which device the device window shows.
const DEVICE_KEY: &str = "adi-new-ui-device";

/// Where the device window's address starts; the device's name follows it.
const DEVICES: &str = "/devices";

/// The apps open in windows, with the number each window goes by — the whole [`AppRef`], since
/// an app's address alone does not say where it answers.
const APPS_KEY: &str = "adi-new-ui-apps";

/// Where an app window's address starts; the app's [`AppRef::key`] follows it.
const APPS: &str = "/apps";

/// The conversations open in windows of their own, with the number each window goes by.
const CHATS_KEY: &str = "adi-new-ui-chats";

/// Where the chat windows' address starts: `/chat` alone is the chat window with its pickers,
/// `/chat/<agent>/<run>` one conversation in a window of its own — `/chat/<node>/<agent>/<run>` when
/// it is on a paired machine.
const CHAT: &str = "/chat";

/// Where every app window's size is kept once one is resized: a new app opens at the size the
/// last one was left at, as a browser opens a new window.
const APP_SIZE_KEY: &str = "adi-new-ui-window-app-size";

/// How much of a window must stay on screen, so a drag can never lose it: this much of its
/// width, and all of its title bar.
const KEEP_VISIBLE: f64 = 96.0;
const TITLEBAR: f64 = 40.0;

/// How close to a screen edge the pointer must come, dragging a window, for the window to snap
/// to it.
const SNAP_EDGE: f64 = 6.0;

/// How far down from the top, or up from the bottom, an edge counts as its corner — where a
/// window snaps to a quarter of the screen rather than a half.
const SNAP_CORNER: f64 = 120.0;

/// The stacking order the windows start from — under the top bar and the island (35), as a
/// window slides under macOS's menu bar and dock, and under the palette (40).
const Z_BASE: usize = 10;

/// A window the new UI can open.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Win {
    Settings,
    About,
    /// A chat with one of this machine's agents.
    Chat,
    /// One paired device's page — which one is [`Desk`]'s to say, so there is one at a time.
    Device,
    /// One app, framed in a small browser. As many as there are apps open, each by the number
    /// [`Desk`] gave it; which app that is is the desk's to say.
    App(u32),
    /// One conversation, pinned: a window that shows that chat and nothing else, opened from a
    /// chat list's "Open in new window". As many as are open, like [`Self::App`].
    Talk(u32),
}

impl Win {
    /// The windows there is one of, drawn from this fixed list. App windows come and go, and are
    /// drawn from [`Desk::apps`].
    pub(super) const FIXED: [Self; 4] = [Self::Settings, Self::About, Self::Chat, Self::Device];

    /// The address that opens it — for the device window, only the start of it.
    fn path(self) -> &'static str {
        match self {
            Self::Settings => "/settings",
            Self::About => "/about",
            Self::Chat => "/chat",
            Self::Device => DEVICES,
            Self::App(_) => APPS,
            Self::Talk(_) => CHAT,
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::About => "About adi",
            Self::Chat | Self::Talk(_) => "Chat",
            Self::Device => "Device",
            Self::App(_) => "App",
        }
    }

    /// Where its position is kept. Not an app window's: each opens cascaded from the last, as a
    /// browser's new windows do, rather than all over one remembered spot.
    fn pos_key(self) -> Option<&'static str> {
        match self {
            Self::Settings => Some("adi-new-ui-window-settings"),
            Self::About => Some("adi-new-ui-window-about"),
            Self::Chat => Some("adi-new-ui-window-chat"),
            Self::Device => Some("adi-new-ui-window-device"),
            Self::App(_) | Self::Talk(_) => None,
        }
    }

    /// Its width in CSS pixels. About is a narrow card, as macOS draws its own; an app gets the
    /// room a page wants.
    fn width(self) -> u16 {
        match self {
            Self::Settings => 480,
            Self::About => 320,
            Self::Chat | Self::Talk(_) => 760,
            Self::Device => 420,
            Self::App(_) => 1024,
        }
    }

    /// The smallest it may be resized to: whatever keeps its contents readable.
    fn min_size(self) -> (f64, f64) {
        match self {
            Self::Settings => (380.0, 240.0),
            Self::About => (280.0, 200.0),
            Self::Chat | Self::Talk(_) => (420.0, 320.0),
            Self::Device => (320.0, 240.0),
            Self::App(_) => (480.0, 320.0),
        }
    }

    /// Where its size is kept, once it has been resized.
    fn size_key(self) -> &'static str {
        match self {
            Self::Settings => "adi-new-ui-window-settings-size",
            Self::About => "adi-new-ui-window-about-size",
            Self::Chat | Self::Talk(_) => "adi-new-ui-window-chat-size",
            Self::Device => "adi-new-ui-window-device-size",
            Self::App(_) => APP_SIZE_KEY,
        }
    }
}

/// What an address names.
enum Place {
    Settings,
    About,
    Chat,
    /// A device, by its name.
    Device(String),
    /// An app, by its [`AppRef::key`].
    App(String),
    /// One conversation, by its machine (`None` for this one), agent and run.
    Talk(Option<String>, String, String),
}

impl Place {
    fn of(path: &str) -> Option<Self> {
        let path = path.trim_end_matches('/');
        // The app's key keeps its `/` (`laptop/notes`), so each part is decoded on its own.
        let named = |prefix: &str| {
            let rest = path.strip_prefix(prefix)?.strip_prefix('/')?;
            (!rest.is_empty()).then(|| {
                rest.split('/')
                    .map(|p| {
                        js_sys::decode_uri_component(p).map_or_else(|_| p.to_string(), String::from)
                    })
                    .collect::<Vec<_>>()
                    .join("/")
            })
        };
        if let Some(name) = named(DEVICES) {
            return Some(Self::Device(name));
        }
        if let Some(key) = named(APPS) {
            return Some(Self::App(key));
        }
        if let Some(key) = named(CHAT) {
            let parts: Vec<&str> = key.split('/').collect();
            return match parts[..] {
                [agent, run] => Some(Self::Talk(None, agent.into(), run.into())),
                [node, agent, run] => Some(Self::Talk(Some(node.into()), agent.into(), run.into())),
                _ => None,
            };
        }
        match path {
            "/settings" => Some(Self::Settings),
            "/about" => Some(Self::About),
            "/chat" => Some(Self::Chat),
            _ => None,
        }
    }
}

/// The app the app window shows: what it is called and where it answers.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct AppRef {
    /// Its name in the window's address: the app's id, after its machine's name when it runs on
    /// a paired machine (`laptop/notes`).
    pub(super) key: String,
    pub(super) name: String,
    /// The paired machine it runs on; `None` for this one.
    pub(super) machine: Option<String>,
    pub(super) url: String,
    /// Its favicon's address, when it was running as it was opened — see `apps::favicon`.
    #[serde(default)]
    pub(super) favicon: Option<String>,
    /// The Lucide icon it names as its picture — see `apps::AppMark`.
    #[serde(default)]
    pub(super) icon: Option<String>,
}

/// The conversation a chat window is pinned to.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct ChatRef {
    /// The paired machine the agent is on; `None` for this one.
    #[serde(default)]
    pub(super) node: Option<String>,
    pub(super) agent: String,
    pub(super) run: String,
    /// What its title bar says — the conversation's own title, as the list it came from had it.
    #[serde(default)]
    pub(super) title: String,
}

/// The open windows and their order.
#[derive(Clone, Copy)]
pub(super) struct Desk {
    /// Back to front: the last one is in front, and is the one the address bar names.
    stack: RwSignal<Vec<Win>>,
    /// The device the device window shows, by the name this machine files it under.
    pub(super) device: RwSignal<Option<String>>,
    /// The apps open in windows, in the order they were opened, each with its window's number.
    pub(super) apps: RwSignal<Vec<(u32, AppRef)>>,
    /// The conversations open in windows of their own, in the order they were opened.
    pub(super) chats: RwSignal<Vec<(u32, ChatRef)>>,
    /// Where the window being dragged will snap if it is let go now — drawn as an outline by
    /// [`SnapPreview`].
    preview: RwSignal<Option<Snap>>,
    /// Whether a window is being moved or resized. An app's page under the pointer would take the
    /// pointer's events for its own document — a capture held by this one does not reach across —
    /// and the window would stop following, so while this is set the pages ignore the pointer.
    pub(super) dragging: RwSignal<bool>,
}

impl Desk {
    /// The windows this device left open, plus whichever one the address names, in front.
    pub(super) fn load() -> Self {
        let mut stack: Vec<Win> = ui::storage()
            .and_then(|s| s.get_item(OPEN_KEY).ok().flatten())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        stack.dedup();
        let device = ui::storage().and_then(|s| s.get_item(DEVICE_KEY).ok().flatten());
        let mut apps: Vec<(u32, AppRef)> = ui::storage()
            .and_then(|s| s.get_item(APPS_KEY).ok().flatten())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        // The two are saved one after the other, so either can outlive the other by a write: a
        // window is only kept when both still know it.
        let mut chats: Vec<(u32, ChatRef)> = ui::storage()
            .and_then(|s| s.get_item(CHATS_KEY).ok().flatten())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        stack.retain(|w| match w {
            Win::App(id) => apps.iter().any(|(a, _)| a == id),
            Win::Talk(id) => chats.iter().any(|(c, _)| c == id),
            _ => true,
        });
        apps.retain(|(id, _)| stack.contains(&Win::App(*id)));
        chats.retain(|(id, _)| stack.contains(&Win::Talk(*id)));
        let desk = Self {
            stack: RwSignal::new(stack),
            device: RwSignal::new(device),
            apps: RwSignal::new(apps),
            chats: RwSignal::new(chats),
            preview: RwSignal::new(None),
            dragging: RwSignal::new(false),
        };
        desk.arrive(&routing::current_path());
        desk
    }

    /// The address changed under us — a load, back, forward. Raise what it names; close nothing,
    /// and write no history, because the browser already has.
    pub(super) fn arrive(self, path: &str) {
        match Place::of(path) {
            Some(Place::Settings) => self.raise(Win::Settings),
            Some(Place::About) => self.raise(Win::About),
            Some(Place::Chat) => self.raise(Win::Chat),
            Some(Place::Device(d)) => {
                self.set_device(d);
                self.raise(Win::Device);
            }
            // An app's address names it but not where it answers, so only an app already open
            // can be come back to by one — any other is an address to nothing.
            Some(Place::App(key)) => {
                if let Some(id) = self.app_id(&key) {
                    self.raise(Win::App(id));
                }
            }
            // A conversation's address says everything its window needs, so it opens one even
            // when none is open — a link to a chat is a chat.
            Some(Place::Talk(node, agent, run)) => {
                let id = self.chat_id(node.as_deref(), &agent, &run);
                let id = id.unwrap_or_else(|| {
                    self.add_chat(ChatRef {
                        node,
                        agent,
                        run,
                        title: String::new(),
                    })
                });
                self.raise(Win::Talk(id));
            }
            None => {}
        }
    }

    /// Open an app in a window of its own, or bring its window forward when it has one — a link
    /// followed, like [`Self::open`].
    pub(super) fn open_app(self, app: AppRef) {
        let id = self.app_id(&app.key).unwrap_or_else(|| {
            let id = self
                .apps
                .with_untracked(|a| a.iter().map(|(id, _)| *id).max().map_or(0, |m| m + 1));
            self.apps.update(|a| a.push((id, app)));
            self.save_apps();
            id
        });
        self.open(Win::App(id));
    }

    /// Open an app in a window of its own even when it already has one — the menu's "Open in new
    /// window", as a browser opens a second window on the same site.
    pub(super) fn open_app_new(self, app: AppRef) {
        let id = self
            .apps
            .with_untracked(|a| a.iter().map(|(id, _)| *id).max().map_or(0, |m| m + 1));
        self.apps.update(|a| a.push((id, app)));
        self.save_apps();
        self.open(Win::App(id));
    }

    /// Open one conversation in a window of its own, or bring its window forward when it has one.
    pub(super) fn open_chat(self, chat: ChatRef) {
        let id = self
            .chat_id(chat.node.as_deref(), &chat.agent, &chat.run)
            .unwrap_or_else(|| self.add_chat(chat));
        self.open(Win::Talk(id));
    }

    fn add_chat(self, chat: ChatRef) -> u32 {
        let id = self
            .chats
            .with_untracked(|c| c.iter().map(|(id, _)| *id).max().map_or(0, |m| m + 1));
        self.chats.update(|c| c.push((id, chat)));
        self.save_chats();
        id
    }

    fn chat_id(self, node: Option<&str>, agent: &str, run: &str) -> Option<u32> {
        self.chats.with_untracked(|c| {
            c.iter()
                .find(|(_, x)| x.node.as_deref() == node && x.agent == agent && x.run == run)
                .map(|(id, _)| *id)
        })
    }

    /// The conversation a chat window shows — tracked.
    pub(super) fn chat(self, id: u32) -> Option<ChatRef> {
        self.chats
            .with(|c| c.iter().find(|(i, _)| *i == id).map(|(_, x)| x.clone()))
    }

    fn save_chats(self) {
        if let (Some(s), Ok(json)) = (
            ui::storage(),
            serde_json::to_string(&self.chats.get_untracked()),
        ) {
            let _ = s.set_item(CHATS_KEY, &json);
        }
    }

    fn app_id(self, key: &str) -> Option<u32> {
        self.apps
            .with_untracked(|a| a.iter().find(|(_, app)| app.key == key).map(|(id, _)| *id))
    }

    /// The app a window shows — tracked.
    pub(super) fn app(self, id: u32) -> Option<AppRef> {
        self.apps
            .with(|a| a.iter().find(|(i, _)| *i == id).map(|(_, app)| app.clone()))
    }

    /// [`Self::app`], untracked.
    pub(super) fn app_untracked(self, id: u32) -> Option<AppRef> {
        self.apps
            .with_untracked(|a| a.iter().find(|(i, _)| *i == id).map(|(_, app)| app.clone()))
    }

    fn save_apps(self) {
        if let (Some(s), Ok(json)) = (
            ui::storage(),
            serde_json::to_string(&self.apps.get_untracked()),
        ) {
            let _ = s.set_item(APPS_KEY, &json);
        }
    }

    /// Open the device window on one device — a link followed, like [`Self::open`].
    pub(super) fn open_device(self, petname: String) {
        self.set_device(petname);
        self.open(Win::Device);
    }

    fn set_device(self, petname: String) {
        if let Some(s) = ui::storage() {
            let _ = s.set_item(DEVICE_KEY, &petname);
        }
        self.device.set(Some(petname));
    }

    /// The title a window's bar shows: the device's name on the device window, the app's (and
    /// its machine's) on the app window.
    pub(super) fn title(self, w: Win) -> String {
        match w {
            Win::Device => self.device.get(),
            Win::App(id) => self.app(id).map(|a| match &a.machine {
                Some(m) => format!("{} — {m}", a.name),
                None => a.name,
            }),
            Win::Talk(id) => self.chat(id).map(|c| {
                let who = match &c.node {
                    Some(n) => format!("{} on {n}", c.agent),
                    None => c.agent,
                };
                if c.title.is_empty() {
                    who
                } else {
                    format!("{} — {who}", c.title)
                }
            }),
            Win::Settings | Win::About | Win::Chat => None,
        }
        .unwrap_or_else(|| w.title().to_string())
    }

    /// The address that names a window as it stands.
    fn address(self, w: Win) -> String {
        let name = match w {
            Win::Device => self.device.get_untracked(),
            Win::App(id) => self.apps.with_untracked(|a| {
                a.iter()
                    .find(|(i, _)| *i == id)
                    .map(|(_, app)| app.key.clone())
            }),
            Win::Talk(id) => self.chats.with_untracked(|c| {
                c.iter()
                    .find(|(i, _)| *i == id)
                    .map(|(_, x)| match &x.node {
                        Some(n) => format!("{n}/{}/{}", x.agent, x.run),
                        None => format!("{}/{}", x.agent, x.run),
                    })
            }),
            Win::Settings | Win::About | Win::Chat => None,
        };
        match name {
            // The key's own `/` is kept, so a paired machine's app reads `/apps/laptop/notes`.
            Some(n) => format!(
                "{}/{}",
                w.path(),
                n.split('/')
                    .map(|part| String::from(js_sys::encode_uri_component(part)))
                    .collect::<Vec<_>>()
                    .join("/")
            ),
            None => w.path().to_string(),
        }
    }

    /// Open a window, or bring it forward — a link followed, so it is a new history entry.
    pub(super) fn open(self, w: Win) {
        self.raise(w);
        self.show_address(true);
    }

    /// Bring a window forward because it was clicked. Not a place visited, so the address is
    /// replaced rather than pushed: back does not step through every click between windows.
    pub(super) fn focus(self, w: Win) {
        if self.stack.with_untracked(|s| s.last() != Some(&w)) {
            self.raise(w);
            self.show_address(false);
        }
    }

    /// Close one window and only that one. An app's window closed is the app closed.
    pub(super) fn close(self, w: Win) {
        self.stack.update(|s| s.retain(|x| *x != w));
        self.save();
        if let Win::App(id) = w {
            self.apps.update(|a| a.retain(|(i, _)| *i != id));
            self.save_apps();
        }
        if let Win::Talk(id) = w {
            self.chats.update(|c| c.retain(|(i, _)| *i != id));
            self.save_chats();
        }
        self.show_address(false);
    }

    /// Close whichever window is in front — what Escape does.
    pub(super) fn close_front(self) {
        if let Some(w) = self.stack.with_untracked(|s| s.last().copied()) {
            self.close(w);
        }
    }

    pub(super) fn is_open(self, w: Win) -> bool {
        self.stack.with(|s| s.contains(&w))
    }

    pub(super) fn is_front(self, w: Win) -> bool {
        self.stack.with(|s| s.last() == Some(&w))
    }

    fn z(self, w: Win) -> usize {
        Z_BASE
            + self
                .stack
                .with(|s| s.iter().position(|x| *x == w).unwrap_or(0))
    }

    fn raise(self, w: Win) {
        self.stack.update(|s| {
            s.retain(|x| *x != w);
            s.push(w);
        });
        self.save();
    }

    fn save(self) {
        if let (Some(s), Ok(json)) = (
            ui::storage(),
            serde_json::to_string(&self.stack.get_untracked()),
        ) {
            let _ = s.set_item(OPEN_KEY, &json);
        }
    }

    /// Point the address bar at the front window, or at `/` when there is none.
    fn show_address(self, push: bool) {
        let to = self
            .stack
            .with_untracked(|s| s.last().copied())
            .map_or_else(|| "/".to_string(), |w| self.address(w));
        if routing::current_path() != to {
            if push {
                routing::push_state(&to);
            } else {
                routing::replace_state(&to);
            }
        }
    }
}

/// Where a window can snap, as macOS and Windows tile them: the whole screen under the top bar, a
/// half of it, or a quarter.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Snap {
    Fill,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Snap {
    /// Where letting go of a dragged window with the pointer at `(x, y)` snaps it: the top edge
    /// fills the screen, a side edge takes that half, and a side edge near a corner that quarter.
    fn at(x: f64, y: f64, top: f64) -> Option<Self> {
        let (vw, vh) = viewport();
        let (left, right) = (x <= SNAP_EDGE, x >= vw - SNAP_EDGE);
        let (high, low) = (y <= top + SNAP_CORNER, y >= vh - SNAP_CORNER);
        match (left, right) {
            (true, _) if high => Some(Self::TopLeft),
            (true, _) if low => Some(Self::BottomLeft),
            (true, _) => Some(Self::Left),
            (_, true) if high => Some(Self::TopRight),
            (_, true) if low => Some(Self::BottomRight),
            (_, true) => Some(Self::Right),
            // The top bar counts as the top edge: the pointer may go into it, the window not.
            _ if y <= top + SNAP_EDGE => Some(Self::Fill),
            _ => None,
        }
    }

    /// Its place, as inline CSS measured from the screen, so it follows the screen when that is
    /// resized. `top` is the top bar's lower edge.
    fn css(self, top: f64) -> String {
        let area = format!("(100dvh - {top:.0}px)");
        let (x, w) = match self {
            Self::Fill => ("0px", "100vw"),
            Self::Left | Self::TopLeft | Self::BottomLeft => ("0px", "50vw"),
            Self::Right | Self::TopRight | Self::BottomRight => ("50vw", "50vw"),
        };
        let (y, h) = match self {
            Self::Fill | Self::Left | Self::Right => (format!("{top:.0}px"), format!("calc{area}")),
            Self::TopLeft | Self::TopRight => (format!("{top:.0}px"), format!("calc({area} / 2)")),
            Self::BottomLeft | Self::BottomRight => (
                format!("calc({top:.0}px + {area} / 2)"),
                format!("calc({area} / 2)"),
            ),
        };
        format!("left: {x}; top: {y}; width: {w}; height: {h}; ")
    }
}

/// The outline of where the window being dragged will snap, under it and over every other
/// window, as macOS draws one.
#[component]
pub(super) fn SnapPreview(desk: Desk, #[prop(into)] top: Signal<f64>) -> impl IntoView {
    move || {
        desk.preview.get().map(|snap| {
            // The front window's own level, drawn before it on the page: under that one, over the
            // rest. The one being dragged is always in front — pressing it brought it there.
            let z = Z_BASE + desk.stack.with(|s| s.len().saturating_sub(1));
            view! {
                <div
                    class="adi-new-snap"
                    style=format!("{}z-index: {z}", snap.css(top.get()))
                    aria-hidden="true"
                ></div>
            }
        })
    }
}

/// A window's frame: a macOS title bar with its traffic lights, dragged to move it — and, let go
/// at a screen edge, snapped to a half, a quarter or the whole screen — or double-clicked to fill
/// the screen (and again to put it back); over a body that scrolls on its own so the title bar —
/// and the way out in it — never scrolls away; and grips on every edge and corner that resize it.
///
/// `top` is the highest a window may go — the top bar's lower edge while it is drawn — so a
/// window can never be dragged or stretched under it, the way macOS keeps windows below its menu
/// bar.
#[component]
pub(super) fn Frame(
    win: Win,
    desk: Desk,
    /// Draw in the light token set, to sit on a light wallpaper.
    #[prop(into)]
    light: Signal<bool>,
    #[prop(into)] top: Signal<f64>,
    children: Children,
) -> impl IntoView {
    // Both `None` until first dragged or resized: the stylesheet centres the window and sizes it
    // to its content, which follows the viewport in a way stored pixels would not.
    let pos = RwSignal::new(
        win.pos_key()
            .and_then(load_pair)
            .map(|(x, y)| clamp_pos(x, y, KEEP_VISIBLE * 2.0, top.get_untracked())),
    );
    let size = RwSignal::new(load_pair(win.size_key()).map(|(w, h)| {
        let (vw, vh) = viewport();
        let (mw, mh) = win.min_size();
        (w.clamp(mw, vw.max(mw)), h.clamp(mh, vh.max(mh)))
    }));
    // Snapped to part of the screen — over, not instead of, the place and size it had, so a
    // double-click, or a drag away, gives those back.
    let snap = RwSignal::new(None::<Snap>);
    let gesture = StoredValue::new(None::<Gesture>);
    let frame: NodeRef<html::Div> = NodeRef::new();
    // Where it sat in the stack when it opened. A window opened over others steps down and right
    // by that many places, as macOS cascades new windows, so none opens exactly over another and
    // hides its title bar. Read once: a window must not jump each time the order changes.
    let cascade = desk
        .stack
        .with_untracked(|s| s.iter().position(|x| *x == win).unwrap_or(0));

    // Take hold of the window for a move (`edge: None`) or a resize from one edge or corner.
    let begin = move |ev: &ev::PointerEvent, edge: Option<Edge>| {
        let (Some(el), Some(handle)) = (
            frame.get(),
            ev.current_target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok()),
        ) else {
            return;
        };
        let r = el.get_bounding_client_rect();
        let rect = (r.left(), r.top(), r.width(), r.height());
        let from = (f64::from(ev.client_x()), f64::from(ev.client_y()));
        let snapped = snap.get_untracked().is_some();
        desk.dragging.set(true);
        // Held to move while snapped: nothing changes until it moves (see `on_move`), and its
        // place and size are left as the ones to give back.
        if snapped && edge.is_none() {
            gesture.set_value(Some(Gesture {
                edge,
                from,
                rect,
                snapped: true,
            }));
        } else {
            // Resized while snapped, it comes out of the snap where it is and grows from there.
            snap.set(None);
            gesture.set_value(Some(Gesture {
                edge,
                from,
                rect,
                snapped: false,
            }));
            pos.set(Some((rect.0, rect.1)));
            if edge.is_some() {
                size.set(Some((rect.2, rect.3)));
            }
        }
        // Captured, so a fast pointer that outruns the handle keeps the gesture going.
        let _ = handle.set_pointer_capture(ev.pointer_id());
        ev.prevent_default();
    };
    let on_move = move |ev: ev::PointerEvent| {
        let Some(mut g) = gesture.get_value() else {
            return;
        };
        // A snapped window that starts to move comes out of the snap and takes back the size it
        // had, under the pointer at the same share of its width, as it does on macOS.
        if g.snapped {
            let (px, py) = (f64::from(ev.client_x()), f64::from(ev.client_y()));
            if (px - g.from.0).hypot(py - g.from.1) < SNAP_EDGE {
                return;
            }
            snap.set(None);
            let w = size
                .get_untracked()
                .map_or_else(|| f64::from(win.width()).min(viewport().0), |(w, _)| w);
            let share = (g.from.0 - g.rect.0) / g.rect.2.max(1.0);
            g.rect = (g.from.0 - share * w, g.rect.1, w, g.rect.3);
            g.snapped = false;
            gesture.set_value(Some(g));
        }
        let (dx, dy) = (
            f64::from(ev.client_x()) - g.from.0,
            f64::from(ev.client_y()) - g.from.1,
        );
        let (x, y, w, h) = g.rect;
        match g.edge {
            None => {
                pos.set(Some(clamp_pos(x + dx, y + dy, w, top.get_untracked())));
                let at = Snap::at(
                    f64::from(ev.client_x()),
                    f64::from(ev.client_y()),
                    top.get_untracked(),
                );
                if desk.preview.get_untracked() != at {
                    desk.preview.set(at);
                }
            }
            Some(edge) => {
                let (x, y, w, h) = resize(
                    (x, y, w, h),
                    edge,
                    (dx, dy),
                    win.min_size(),
                    top.get_untracked(),
                );
                pos.set(Some((x, y)));
                size.set(Some((w, h)));
            }
        }
    };
    let on_up = move |_: ev::PointerEvent| {
        if let Some(g) = gesture.get_value() {
            gesture.set_value(None);
            desk.dragging.set(false);
            // Let go over a snap: it snaps, and keeps the place it was taken from as the one a
            // double-click or a drag away gives back.
            if let Some(at) = desk.preview.get_untracked() {
                desk.preview.set(None);
                snap.set(Some(at));
                pos.set(Some((g.rect.0, g.rect.1)));
            }
            if let Some(key) = win.pos_key() {
                save_pair(key, pos.get_untracked());
            }
            save_pair(win.size_key(), size.get_untracked());
        }
    };

    let on_bar_down = move |ev: ev::PointerEvent| {
        // The traffic lights sit in the title bar; pressing one is a click, not a drag.
        let on_button = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|t| t.closest("button").ok().flatten())
            .is_some();
        if !on_button && ev.button() == 0 {
            begin(&ev, None);
        }
    };
    let on_bar_double = move |ev: ev::MouseEvent| {
        let on_button = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|t| t.closest("button").ok().flatten())
            .is_some();
        if !on_button {
            snap.update(|s| *s = if s.is_some() { None } else { Some(Snap::Fill) });
        }
    };

    view! {
        <div
            node_ref=frame
            class="adi-new-win"
            class:light=move || light.get()
            class:is-placed=move || pos.get().is_some()
            class:is-sized=move || size.get().is_some()
            class:is-front=move || desk.is_front(win)
            class:is-snapped=move || snap.get().is_some()
            class:is-filled=move || snap.get() == Some(Snap::Fill)
            style=move || {
                // Flush to the screen's edges: the stylesheet takes off the corners, the lift and
                // the centring.
                if let Some(s) = snap.get() {
                    return format!("{}z-index: {}", s.css(top.get()), desk.z(win));
                }
                // Kept below the top bar here too, not only while dragging: switching the bar on
                // must push down a window already sitting where it now goes.
                let at = pos
                    .get()
                    .map(|(x, y)| format!("left: {x:.0}px; top: {:.0}px; ", y.max(top.get())))
                    .unwrap_or_default();
                let wh = size
                    .get()
                    .map(|(w, h)| format!("width: {w:.0}px; height: {h:.0}px; "))
                    .unwrap_or_default();
                format!(
                    "{at}{wh}z-index: {}; --cascade: {cascade}; --win-w: {}px",
                    desk.z(win),
                    win.width(),
                )
            }
            role="dialog"
            aria-label=move || desk.title(win)
            // Any press inside a window brings it forward, as a click on a macOS window does.
            on:pointerdown=move |_| desk.focus(win)
        >
            <header
                class="adi-new-win__bar"
                on:pointerdown=on_bar_down
                on:dblclick=on_bar_double
                on:pointermove=on_move
                on:pointerup=on_up
                on:pointercancel=on_up
            >
                // macOS's three lights. Only close does anything yet — nothing here minimises or
                // zooms — so the other two are drawn disabled, the way macOS draws them on a
                // window that cannot.
                <div class="adi-new-win__lights">
                    <button
                        class="adi-new-win__light adi-new-win__light--close"
                        type="button"
                        aria-label=move || format!("Close {}", desk.title(win))
                        title="Close"
                        on:click=move |_| desk.close(win)
                    ></button>
                    <span class="adi-new-win__light" aria-hidden="true"></span>
                    <span class="adi-new-win__light" aria-hidden="true"></span>
                </div>
                <h1 class="adi-new-win__title">{move || desk.title(win)}</h1>
            </header>
            <div class="adi-new-win__body">{children()}</div>
            {Edge::ALL
                .into_iter()
                .map(|edge| view! {
                    <div
                        class=format!("adi-new-win__grip adi-new-win__grip--{}", edge.name())
                        aria-hidden="true"
                        on:pointerdown=move |ev: ev::PointerEvent| {
                            if ev.button() == 0 {
                                begin(&ev, Some(edge));
                            }
                        }
                        on:pointermove=on_move
                        on:pointerup=on_up
                        on:pointercancel=on_up
                    ></div>
                })
                .collect_view()}
        </div>
    }
}

/// A move or a resize in progress.
#[derive(Clone, Copy)]
struct Gesture {
    /// `None` for a move.
    edge: Option<Edge>,
    /// Where the pointer went down.
    from: (f64, f64),
    /// The window's `(x, y, width, height)` when it did.
    rect: (f64, f64, f64, f64),
    /// Taken hold of to move while snapped, and not yet moved: it stays snapped until it is, so
    /// a click — or the first half of a double-click — on its title bar leaves it where it is.
    snapped: bool,
}

/// An edge or a corner a window is resized from.
#[derive(Clone, Copy)]
struct Edge {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

impl Edge {
    const fn new(top: bool, right: bool, bottom: bool, left: bool) -> Self {
        Self {
            left,
            right,
            top,
            bottom,
        }
    }

    const ALL: [Self; 8] = [
        Self::new(true, false, false, false),
        Self::new(false, true, false, false),
        Self::new(false, false, true, false),
        Self::new(false, false, false, true),
        Self::new(true, true, false, false),
        Self::new(false, true, true, false),
        Self::new(false, false, true, true),
        Self::new(true, false, false, true),
    ];

    /// `n`, `se` and so on — the compass point the stylesheet places and cursors it by.
    fn name(self) -> &'static str {
        match (self.top, self.right, self.bottom, self.left) {
            (true, false, false, false) => "n",
            (false, true, false, false) => "e",
            (false, false, true, false) => "s",
            (false, false, false, true) => "w",
            (true, true, false, false) => "ne",
            (false, true, true, false) => "se",
            (false, false, true, true) => "sw",
            _ => "nw",
        }
    }
}

/// The window's new `(x, y, width, height)` after the pointer moved `(dx, dy)` from where it took
/// hold of `edge`. The edge opposite the one held stays where it is — shrinking past the minimum
/// stops the moving edge rather than pushing the window along — and no edge passes the viewport,
/// or `top` above.
fn resize(
    (x, y, w, h): (f64, f64, f64, f64),
    edge: Edge,
    (dx, dy): (f64, f64),
    (min_w, min_h): (f64, f64),
    top: f64,
) -> (f64, f64, f64, f64) {
    let (vw, vh) = viewport();
    let (mut x, mut y, mut w, mut h) = (x, y, w, h);
    if edge.right {
        w = (w + dx).clamp(min_w, (vw - x).max(min_w));
    }
    if edge.left {
        let right = x + w;
        x = (x + dx).clamp(0.0, right - min_w);
        w = right - x;
    }
    if edge.bottom {
        h = (h + dy).clamp(min_h, (vh - y).max(min_h));
    }
    if edge.top {
        let bottom = y + h;
        y = (y + dy).clamp(top, (bottom - min_h).max(top));
        h = bottom - y;
    }
    (x, y, w, h)
}

/// Keep a window of this width with enough of it on screen to take hold of again, and its title
/// bar no higher than `top`.
fn clamp_pos(x: f64, y: f64, width: f64, top: f64) -> (f64, f64) {
    let (vw, vh) = viewport();
    (
        x.clamp(KEEP_VISIBLE - width, (vw - KEEP_VISIBLE).max(0.0)),
        y.clamp(top, (vh - TITLEBAR).max(top)),
    )
}

/// The screen's height, for a menu deciding whether it has room to drop down.
pub(super) fn viewport_height() -> f64 {
    viewport().1
}

fn viewport() -> (f64, f64) {
    let w = window();
    let px = |v: Result<wasm_bindgen::JsValue, _>| v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
    (px(w.inner_width()), px(w.inner_height()))
}

/// A saved `a,b` pair — a position or a size.
fn load_pair(key: &str) -> Option<(f64, f64)> {
    let saved = ui::storage()?.get_item(key).ok()??;
    let (a, b) = saved.split_once(',')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

fn save_pair(key: &str, pair: Option<(f64, f64)>) {
    if let (Some(s), Some((a, b))) = (ui::storage(), pair) {
        let _ = s.set_item(key, &format!("{a:.0},{b:.0}"));
    }
}
