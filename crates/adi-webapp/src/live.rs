//! The live channel: one websocket per tab, in place of a timer per thing being looked at.
//!
//! The control panel used to ask the backend the same questions on a loop — a chat's transcript
//! and run list every second, the rails every four — and throw nearly every answer away for being
//! identical to the last. This says what the page is looking at *once*, and the server sends an
//! answer back only when it actually differs. See `adi-app`'s `live` module for the other half.
//!
//! A subscription ([`Sub`]) is not a new kind of endpoint: it is the very request the page used to
//! poll, plus what to do with the answer. That is what makes this a change of transport rather
//! than a second API to keep in step with the first — [`fetch`](crate::fetch) still serves every
//! one-off read and every mutation, unchanged.
//!
//! The socket is a fast path, never a requirement: while it is down, [`connected`] reports false
//! and the shell's timers poll exactly as they did before.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use leptos::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;
use web_sys::{CloseEvent, MessageEvent, WebSocket};

/// How long to wait before the first reconnect, doubling up to [`MAX_BACKOFF`].
const BASE_BACKOFF: u32 = 500;

/// The longest gap between reconnect attempts — a backend restarted while a tab sits open should
/// be picked up promptly, and an unreachable one shouldn't be hammered.
const MAX_BACKOFF: u32 = 10_000;

/// What to do with one answer: called with its raw JSON. Boxed rather than generic so
/// subscriptions to a dozen different types live in one list.
type Apply = Rc<dyn Fn(&str)>;

/// One read this page is watching: the request, and what to do each time its answer changes.
pub(crate) struct Sub {
    method: &'static str,
    path: String,
    body: String,
    apply: Apply,
}

impl Sub {
    /// Watch a `GET` endpoint, deserializing each answer into `T`. Always local — see
    /// [`Self::get_on`] for a read that follows a specific source (`docs/fleet.md` §13).
    pub(crate) fn get<T, F>(path: impl Into<String>, apply: F) -> Self
    where
        T: DeserializeOwned,
        F: Fn(T) + 'static,
    {
        Self::new(None, "GET", path.into(), String::new(), apply)
    }

    /// [`Self::get`], routed at a specific paired node (or this machine, for `None`) — the sessions
    /// rail's per-source merge and the open conversation's own watches, which may be on any selected
    /// source at once and so can never share the single implicit routing the first cut of this used.
    pub(crate) fn get_on<T, F>(node: Option<&str>, path: impl Into<String>, apply: F) -> Self
    where
        T: DeserializeOwned,
        F: Fn(T) + 'static,
    {
        Self::new(node, "GET", path.into(), String::new(), apply)
    }

    /// Watch one of the reads that is a `POST` because it carries a subject — an agent name, a
    /// run id — in its body. Always local; see [`Self::post_on`].
    pub(crate) fn post<T, B, F>(path: impl Into<String>, body: &B, apply: F) -> Self
    where
        T: DeserializeOwned,
        B: Serialize,
        F: Fn(T) + 'static,
    {
        let body = serde_json::to_string(body).unwrap_or_else(|_| "{}".to_string());
        Self::new(None, "POST", path.into(), body, apply)
    }

    /// [`Self::post`], routed at a specific paired node (or this machine, for `None`).
    pub(crate) fn post_on<T, B, F>(
        node: Option<&str>,
        path: impl Into<String>,
        body: &B,
        apply: F,
    ) -> Self
    where
        T: DeserializeOwned,
        B: Serialize,
        F: Fn(T) + 'static,
    {
        let body = serde_json::to_string(body).unwrap_or_else(|_| "{}".to_string());
        Self::new(node, "POST", path.into(), body, apply)
    }

    fn new<T, F>(node: Option<&str>, method: &'static str, path: String, body: String, apply: F) -> Self
    where
        T: DeserializeOwned,
        F: Fn(T) + 'static,
    {
        Self {
            method,
            // Through the same mapping the fetch this replaces takes, so a subscription and its
            // one-off equivalent can never be pointed at two different machines
            // (`crate::fetch::routed_for`). The server keys a topic by its path, so a node's read is
            // its own topic and two tabs watching two different sources get an answer each.
            path: crate::fetch::routed_for(node, &path),
            body,
            // A payload that won't parse is dropped rather than surfaced: it means the server and
            // this bundle disagree about a type, which a reload fixes and a flash message doesn't.
            apply: Rc::new(move |json| {
                if let Ok(value) = serde_json::from_str::<T>(json) {
                    apply(value);
                }
            }),
        }
    }

    /// How the server names this read — the same `method path\nbody` it keys its own topics by.
    fn key(&self) -> String {
        format!("{} {}\n{}", self.method, self.path, self.body)
    }
}

/// Everything the socket owns. Lives in a thread-local because wasm is single-threaded and this
/// is per-tab state, not per-component: the socket outlives any view that subscribes through it.
struct Live {
    socket: Option<WebSocket>,
    /// What this page is watching, by topic key. A `BTreeMap` so the message sent to the server is
    /// stable for a given set of subscriptions.
    subs: BTreeMap<String, Apply>,
    /// How long the next reconnect waits.
    backoff: u32,
    /// Held so the timer isn't cancelled by being dropped.
    retry: Option<Timeout>,
}

thread_local! {
    static LIVE: RefCell<Live> = RefCell::new(Live {
        socket: None,
        subs: BTreeMap::new(),
        backoff: BASE_BACKOFF,
        retry: None,
    });

    /// Whether the socket is up — read by the shell to decide whether its fallback timers need to
    /// do anything, and by the header to say so.
    static CONNECTED: RwSignal<bool> = RwSignal::new(false);

    /// Run whenever an answer arrives, whatever it was about. The shell uses it for the "updated
    /// Ns ago" label, which asks when the backend last said anything — not what it said.
    static HEARD: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Register what to do each time the backend says anything at all.
pub(crate) fn on_message(heard: impl Fn() + 'static) {
    HEARD.with(|slot| *slot.borrow_mut() = Some(Rc::new(heard)));
}

/// Whether the live channel is carrying this page's updates. `false` means the shell's polling
/// fallback is what's keeping the page fresh.
///
/// Read untracked: the callers are the fallback timers, which ask what is true right now. Reading
/// it tracked inside an effect would rebuild that effect on every reconnect.
pub(crate) fn connected() -> bool {
    CONNECTED.with(RwSignal::get_untracked)
}

/// Open the live channel. Call once, as the app starts; reconnects handle themselves from there.
pub(crate) fn start() {
    connect();
}

/// Replace what this page is watching.
///
/// The whole set, every time — the page describes what it is showing now, and anything absent
/// stops being watched (and, if nobody else wants it, stops being computed on the server). Sends
/// nothing when the set is unchanged, so this is safe to call from an effect that re-runs often.
pub(crate) fn watch(subs: Vec<Sub>) {
    let changed = LIVE.with(|live| {
        let mut live = live.borrow_mut();
        let mut next = BTreeMap::new();
        for sub in subs {
            next.insert(sub.key(), sub.apply);
        }
        // Compare the keys, not the closures: two subscriptions to the same read are the same
        // subscription however the callback was built.
        let changed = !next.keys().eq(live.subs.keys());
        live.subs = next;
        changed
    });
    if changed {
        send_subscription();
    }
}

/// Tell the server the current watch list. A no-op while the socket is down — [`on_open`] sends it
/// as soon as one is up, which is also how a reconnect restores what the page was watching.
fn send_subscription() {
    LIVE.with(|live| {
        let live = live.borrow();
        let Some(socket) = live.socket.as_ref() else {
            return;
        };
        if socket.ready_state() != WebSocket::OPEN {
            return;
        }
        let subs = live
            .subs
            .keys()
            .filter_map(|key| {
                // Unpick the key back into the request it names: "METHOD path\nbody".
                let (head, body) = key.split_once('\n')?;
                let (method, path) = head.split_once(' ')?;
                Some(serde_json::json!({
                    "method": method,
                    "path": path,
                    "body": body,
                }))
            })
            .collect::<Vec<_>>();
        let message = serde_json::json!({ "sub": subs }).to_string();
        let _ = socket.send_with_str(&message);
    });
}

/// The live channel's address: this same origin, `ws://` or `wss://` to match the page.
fn endpoint() -> Option<String> {
    let location = web_sys::window()?.location();
    let secure = location.protocol().ok()? == "https:";
    let scheme = if secure { "wss" } else { "ws" };
    Some(format!("{scheme}://{}/api/ws", location.host().ok()?))
}

/// Open a socket and wire up its callbacks.
fn connect() {
    let Some(url) = endpoint() else {
        return;
    };
    let Ok(socket) = WebSocket::new(&url) else {
        schedule_reconnect();
        return;
    };

    let on_open = Closure::<dyn FnMut()>::new(move || {
        CONNECTED.with(|c| c.set(true));
        LIVE.with(|live| live.borrow_mut().backoff = BASE_BACKOFF);
        // Whatever the page is watching — on a first connection the shell's effect has already
        // filled this in, and on a reconnect it is what was being watched when the socket died.
        send_subscription();
    });
    socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    on_open.forget();

    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        if let Some(text) = event.data().as_string() {
            deliver(&text);
        }
    });
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();

    // Both failure paths land on the same handling: the socket is gone, so try again. `onerror`
    // is always followed by `onclose`, so only the latter reconnects — otherwise one failed
    // connection would start two.
    let on_close = Closure::<dyn FnMut(CloseEvent)>::new(move |_| {
        CONNECTED.with(|c| c.set(false));
        LIVE.with(|live| live.borrow_mut().socket = None);
        schedule_reconnect();
    });
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();

    LIVE.with(|live| live.borrow_mut().socket = Some(socket));
}

/// Hand one server message to whoever subscribed to it.
fn deliver(text: &str) {
    let Ok(serde_json::Value::Object(message)) = serde_json::from_str::<serde_json::Value>(text)
    else {
        return;
    };
    let Some(key) = message.get("key").and_then(serde_json::Value::as_str) else {
        return;
    };
    // Cloned out before calling, so the thread-local isn't borrowed while a handler runs.
    if let Some(heard) = HEARD.with(|slot| slot.borrow().clone()) {
        heard();
    }
    // A read that failed server-side keeps the last good value on screen rather than blanking the
    // page; the next successful answer replaces it.
    if message.get("status").and_then(serde_json::Value::as_u64) != Some(200) {
        return;
    }
    let Some(data) = message.get("data") else {
        return;
    };
    // Cloned out of the map before calling, so the thread-local isn't borrowed while a handler
    // runs — a handler writes signals, and a signal write can reach code that subscribes.
    let apply = LIVE.with(|live| live.borrow().subs.get(key).cloned());
    if let Some(apply) = apply {
        apply(&data.to_string());
    }
}

/// Try again later, backing off — a backend that is down (or restarting after a deploy) shouldn't
/// be met with a reconnect per frame.
fn schedule_reconnect() {
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        let delay = live.backoff;
        live.backoff = (delay * 2).min(MAX_BACKOFF);
        live.retry = Some(Timeout::new(delay, connect));
    });
}
