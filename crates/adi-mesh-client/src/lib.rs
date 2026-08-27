//! **The browser mesh client** — a tab that is its own iroh peer, pairs with adi nodes, and opens
//! each node's control panel and the dashboards it runs from its own origin (`docs/fleet.md` §12,
//! ADI-13).
//!
//! There is no server here. The page is static files; everything it shows it fetched itself over
//! QUIC, from a machine that is listening on nothing. What that costs and what it buys:
//!
//! * **It dials and never accepts** ([`mesh`]). No ALPN is registered on the endpoint, so nothing
//!   can open a stream to this tab — which also means nothing can *dial* it, and that is what
//!   decides the pairing direction: the tab spends an invite the node minted, rather than the node
//!   dialling a viewer as `docs/fleet.md` §8 first described. See [`invite`].
//! * **The identity is browser storage** ([`store`]). The secret key and every node's password
//!   live in `IndexedDB` on this origin and leave it never. Clearing site data destroys the pairing;
//!   there is no Keychain here.
//! * **Everything is relayed** for the whole session, because a browser has no UDP socket. The
//!   relay is therefore a hard precondition and not a latency question — [`mesh::HOME_RELAY`].
//!
//! A machine's pages are served by intercepting `fetch` in a service worker and answering from the
//! mesh ([`bridge`]), plus a shim for the one thing a service worker cannot see: `new WebSocket()`
//! ([`ws`]). Which of a machine's services a page belongs to is in the path the worker keys on, and
//! what it runs is asked of the machine itself ([`dashboards`]).

use wasm_bindgen::prelude::*;

pub mod bridge;
pub mod dashboards;
pub mod http;
pub mod invite;
/// Private: the mark is drawn by this shell and by nothing else, and a component exported from a
/// wasm bundle is a component with an API to keep.
mod mark;
pub mod mesh;
pub mod probe;
pub mod scan;
pub mod store;
pub mod ui;
pub mod ws;

/// Start the client: the panic hook, iroh's logging, and the shell.
///
/// Everything asynchronous — reading the key out of `IndexedDB`, binding the endpoint — happens
/// inside the shell, so the list renders before the relay session is up rather than after it.
#[wasm_bindgen(start)]
pub fn start() {
    init_logging();
    ui::mount();
}

/// The mesh wire protocol, verbatim from the crate that owns it.
///
/// Not a copy — a copy is a thing that drifts, and a client that wrote a re-typed frame would
/// prove nothing about the frame the Mac front door and the iOS viewer write. It compiles for
/// wasm unchanged because it is generic over tokio's IO traits, and iroh's QUIC streams implement
/// those in a browser exactly as they do natively.
#[path = "../../adi-mesh/src/protocol.rs"]
#[allow(
    dead_code,
    reason = "the file is included whole; a dial-only client uses about half of it"
)]
// Without this, `cargo fmt` run in *this* crate follows the include and rewrites a tracked file in
// another crate — it has done exactly that once. The skip leaves the file to the crate that owns it.
#[rustfmt::skip]
pub mod protocol;

/// `N` bytes from the platform's random source.
///
/// On wasm this reaches `crypto.getRandomValues` — the same source `iroh::SecretKey::generate`
/// draws from, which is what makes minting an identity in a tab a real key and not a toy.
///
/// # Panics
/// Only if the platform has no random source, which is not a condition anything here can
/// meaningfully continue past: every caller is either minting an identity or masking a frame.
#[must_use]
pub fn random_bytes<const N: usize>() -> [u8; N] {
    use rand::TryRng as _;
    let mut bytes = [0u8; N];
    rand::rngs::SysRng
        .try_fill_bytes(&mut bytes)
        .expect("the platform random source is unavailable");
    bytes
}

/// Milliseconds since the page loaded, from `performance.now()` — a duration clock, not a wall
/// clock, so a system time change cannot make a measurement negative.
#[must_use]
pub fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now())
}

/// Print to the browser console. The only log this transport has in a reader's hands.
pub fn log(message: &str) {
    web_sys::console::log_1(&JsValue::from_str(message));
}

/// Install the panic hook and route iroh's `tracing` to the console.
///
/// Kept in the shipped bundle rather than behind a debug flag: a reader's failed dial happens on a
/// phone, on a network we cannot see, and "open the console and send me what it says" is the only
/// diagnostic that will ever reach us. `INFO` and not `DEBUG` — the relay handshake at `DEBUG` is
/// thousands of lines a minute.
pub fn init_logging() {
    console_error_panic_hook::set_once();
    let _ = wasm_tracing::set_as_global_default_with_config(
        wasm_tracing::WasmLayerConfig::new()
            .set_max_level(tracing::Level::INFO)
            .to_owned(),
    );
}
