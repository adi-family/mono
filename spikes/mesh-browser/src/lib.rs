//! Spike: a browser tab as its own iroh peer, speaking `adi/mesh/http/1` to an adi node.
//!
//! This is the one question ADI-13 stands or falls on, and nothing else: **does an
//! [`iroh::Endpoint`] compiled to `wasm32-unknown-unknown` bind, dial a node by key over a relay,
//! and carry the mesh's own HTTP gateway protocol?** There is no UI here, no key store, no service
//! worker and no catalog — those are all cheap once the transport is real, and all worthless if it
//! is not.
//!
//! Two things make the answer trustworthy rather than encouraging:
//!
//! - The wire protocol is **not reimplemented**. `protocol.rs` below is `#[path]`-included straight
//!   out of `crates/adi-mesh/src/protocol.rs`, so the frame this tab writes is byte for byte the
//!   frame the iOS viewer and the Mac front door write, and it stays that way when that file
//!   changes. Its functions are generic over tokio's IO traits, and iroh's QUIC streams implement
//!   them on wasm exactly as they do natively (noq's impls are not feature-gated) — that
//!   compatibility is itself part of what the spike proves.
//! - The far side is a **real `adi-mesh` daemon** (`run.sh` starts one against a scratch store), so
//!   the answer comes back through the production `gateway::serve_peer` path: grants checked, Basic
//!   auth enforced, the route table consulted.
//!
//! What a browser cannot do is accept: there is no `alpns()` call here and no accept loop. It dials
//! and nothing else, which is exactly the shape ADI-13 asks for.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey};
use serde::Serialize;
// `write_all` on a `SendStream` is iroh's own inherent method, not the tokio trait's — the same
// call the iOS viewer makes. Only the read side needs a trait import here.
use tokio::io::AsyncReadExt as _;
use wasm_bindgen::prelude::*;

/// The mesh wire protocol, verbatim from the crate that owns it. Not a copy — a copy is a thing
/// that drifts, and a spike that proves a re-typed frame proves nothing about the real one.
#[path = "../../../crates/adi-mesh/src/protocol.rs"]
#[allow(
    dead_code,
    reason = "the file is included whole; a dialer uses about half of it"
)]
// Without this, `cargo fmt` run in *this* crate follows the include and rewrites a tracked file in
// the tree — it did exactly that once. The skip leaves the borrowed file to the crate that owns it.
#[rustfmt::skip]
mod protocol;

use protocol::HttpStatus;

/// The most of a node's reply this spike will read. A dashboard's HTML fits many times over; the
/// cap is here so a misrouted stream cannot make the tab buffer without bound.
const MAX_REPLY: u64 = 4 * 1024 * 1024;

/// How long any one step may take before it is reported as a hang rather than waited on.
///
/// Generous, because every byte of this goes through a relay (a browser has no UDP, so there is no
/// hole punching to fall back on) and because the first step also includes the relay's own TLS and
/// websocket handshake.
const STEP_TIMEOUT: Duration = Duration::from_secs(20);

/// How long to wait for this tab's *own* home relay before dialling regardless. See where it is
/// used: a viewer is never dialled, so its home relay is a convenience, not a precondition.
const HOME_RELAY_WAIT: Duration = Duration::from_secs(8);

/// What the page renders and posts back to the collector.
#[derive(Debug, Serialize)]
struct Report {
    /// Whether every step completed. The page's exit status, in one boolean.
    ok: bool,
    /// The step that failed, when one did.
    error: Option<String>,
    /// This tab's own iroh key — what a node's operator would authorize.
    browser_key: Option<String>,
    /// The node's answer to the gateway frame (`ok`, `no such service on that node`, …).
    mesh_status: Option<String>,
    /// The HTTP status line the node's service replied with, through the mesh.
    http_status: Option<u16>,
    /// The first bytes of the body, so a human can see it really is the service's own page.
    body_preview: Option<String>,
    /// Every step with the milliseconds it completed at, so a hang is visible as a gap.
    steps: Vec<String>,
}

/// A step log with millisecond stamps from the page's own clock.
struct Log {
    started: f64,
    steps: Vec<String>,
}

impl Log {
    fn new() -> Self {
        Self {
            started: now(),
            steps: Vec::new(),
        }
    }

    fn say(&mut self, message: &str) {
        let line = format!("[{:>7.0}ms] {message}", now() - self.started);
        web_sys::console::log_1(&JsValue::from_str(&line));
        self.steps.push(line);
    }
}

/// The page's clock. `performance.now()` rather than `Date::now`, because these are durations.
fn now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now())
}

/// Dial `node` over `relay` and fetch `path` from its `service`, over the mesh. Returns the
/// [`Report`] as JSON — the page renders it and posts it to the collector.
///
/// Never returns `Err`: a failure is a result, and a spike that throws it away as a JS exception
/// loses the step log that says *where* it failed.
#[wasm_bindgen]
pub async fn run_spike(
    secret: String,
    node: String,
    relay: String,
    home_relay: String,
    service: String,
    username: String,
    password: String,
    path: String,
) -> String {
    console_error_panic_hook::set_once();
    // iroh's own view of the relay handshake and the QUIC path. Without it a failed dial is one
    // error string with no story behind it.
    let _ = wasm_tracing::set_as_global_default_with_config(
        wasm_tracing::WasmLayerConfig::new()
            .set_max_level(tracing::Level::DEBUG)
            .to_owned(),
    );

    let mut log = Log::new();
    let mut report = Report {
        ok: false,
        error: None,
        browser_key: None,
        mesh_status: None,
        http_status: None,
        body_preview: None,
        steps: Vec::new(),
    };

    match attempt(
        &mut log,
        &mut report,
        &secret,
        &node,
        &relay,
        &home_relay,
        &service,
        &username,
        &password,
        &path,
    )
    .await
    {
        Ok(()) => {
            report.ok = true;
            log.say("SPIKE PASSED");
        }
        Err(e) => {
            report.error = Some(e.clone());
            log.say(&format!("SPIKE FAILED: {e}"));
        }
    }
    report.steps = std::mem::take(&mut log.steps);
    serde_json::to_string_pretty(&report).unwrap_or_else(|e| {
        format!("{{\"ok\":false,\"error\":\"could not serialise the report: {e}\"}}")
    })
}

/// The spike proper, one step at a time. Every error is a `String` because the only consumer is a
/// human reading a page.
#[expect(clippy::too_many_arguments, reason = "a spike entry point, not an API")]
async fn attempt(
    log: &mut Log,
    report: &mut Report,
    secret: &str,
    node: &str,
    relay: &str,
    home_relay: &str,
    service: &str,
    username: &str,
    password: &str,
    path: &str,
) -> Result<(), String> {
    // --- 1. an identity, minted in the tab -------------------------------------------------
    let node: EndpointId = node
        .trim()
        .parse()
        .map_err(|e| format!("the node key does not parse as an EndpointId: {e}"))?;
    let relay: RelayUrl = relay
        .trim()
        .parse()
        .map_err(|e| format!("the relay does not parse as a URL: {e}"))?;
    // The relay map this tab calls *home*, as against the one the node is reached through. They
    // are separable on purpose: a client shipped from one domain to everybody cannot know which
    // relay each reader's machine uses, so whether the two have to match decides whether the
    // browser stores a relay per node or simply carries a default.
    let home: RelayUrl = match home_relay.trim() {
        "" => relay.clone(),
        url => url
            .parse()
            .map_err(|e| format!("the home relay does not parse as a URL: {e}"))?,
    };

    // Minted in the tab when nothing is passed in — which is the spike's key story in miniature:
    // ADI-13 puts this in IndexedDB, and that it can be generated at all in a browser is part of
    // what is being checked, since `SecretKey` pulls its randomness from `getrandom`, which
    // reaches `crypto.getRandomValues` on wasm.
    //
    // A key can also be *handed in*, and the harness does that for one reason: a node authorizes a
    // peer by key (`docs/fleet.md` §2), so the key has to exist in the node's registry before the
    // tab dials. A generated one would only ever be learnt after the dial it was needed for.
    let secret = match hex32(secret) {
        Some(bytes) => SecretKey::from_bytes(&bytes),
        None if secret.trim().is_empty() => SecretKey::generate(),
        None => return Err("the secret key must be 64 hex characters, or empty".into()),
    };
    let me = secret.public();
    report.browser_key = Some(me.to_string());
    log.say(&format!("this tab's key is {me}"));

    // --- 2. bind an endpoint, in a browser --------------------------------------------------
    //
    // `presets::Minimal` and not `presets::N0`: N0 adds a pkarr *publisher*, which announces this
    // tab's address to n0's DNS. A viewer is dialled by nobody, so publishing is a leak with no
    // upside — and with the relay named explicitly below, no address lookup is needed either.
    //
    // No `.alpns(…)`: a browser dials and never accepts.
    let endpoint = with_timeout(
        "binding the endpoint",
        Endpoint::builder(presets::Minimal)
            .secret_key(secret)
            .relay_mode(RelayMode::Custom(RelayMap::from_iter([home.clone()])))
            .bind(),
    )
    .await?
    .map_err(|e| format!("binding the endpoint failed: {e}"))?;
    log.say("endpoint bound");

    // A browser has no UDP socket, so a relay session is the *only* path — waiting for one here
    // turns "connect timed out" into "the relay never came up", which are different bugs.
    //
    // Not fatal, though, and that distinction is the point: `online()` waits for the relay this tab
    // calls *home*, which is where a peer would reach **it**. Nothing dials a browser. If the home
    // relay never comes up but the node's does, the dial below should still work — so a timeout
    // here is logged and the spike carries on to find out.
    match n0_future::time::timeout(HOME_RELAY_WAIT, endpoint.online()).await {
        Ok(()) => log.say(&format!("home relay session up ({home})")),
        Err(_) => log.say(&format!(
            "no home relay session after {HOME_RELAY_WAIT:?} ({home}) — dialling anyway"
        )),
    }

    // --- 3. dial the node by key ------------------------------------------------------------
    //
    // The relay URL travels with the address rather than being looked up: it is what ADI-13 says
    // the browser stores per node, and it keeps the spike honest about what a paired browser knows.
    let addr = EndpointAddr::new(node).with_relay_url(relay);
    let conn = with_timeout(
        "dialling the node",
        endpoint.connect(addr, protocol::HTTP_ALPN),
    )
    .await?
    .map_err(|e| format!("dialling the node failed: {e}"))?;
    log.say(&format!(
        "connected to {node} on {}",
        String::from_utf8_lossy(protocol::HTTP_ALPN)
    ));

    // --- 4. the gateway frame ---------------------------------------------------------------
    let (mut send, mut recv) = with_timeout("opening a bi-stream", conn.open_bi())
        .await?
        .map_err(|e| format!("opening a stream failed: {e}"))?;

    protocol::write_http_request(&mut send, service)
        .await
        .map_err(|e| format!("writing the service frame failed: {e}"))?;
    log.say(&format!("wrote the `{service}` service frame"));

    let status = with_timeout(
        "reading the mesh status byte",
        protocol::read_http_status(&mut recv),
    )
    .await?
    .map_err(|e| format!("reading the mesh status failed: {e}"))?;
    report.mesh_status = Some(status.reason().to_string());
    log.say(&format!("node answered: {}", status.reason()));
    if status != HttpStatus::Ok {
        return Err(format!("the node refused the request: {}", status.reason()));
    }

    // --- 5. an ordinary HTTP request, over QUIC over a relay --------------------------------
    let head = head(path, (username, password));
    send.write_all(&head)
        .await
        .map_err(|e| format!("writing the request head failed: {e}"))?;
    // Says the request is complete, which is what makes the node's splice shut its upstream write
    // half — without it the service sits waiting for a body that is never coming.
    send.finish()
        .map_err(|e| format!("finishing the request failed: {e}"))?;
    log.say(&format!("sent GET {path}"));

    let mut raw = Vec::new();
    with_timeout(
        "reading the node's reply",
        recv.take(MAX_REPLY).read_to_end(&mut raw),
    )
    .await?
    .map_err(|e| format!("reading the reply failed: {e}"))?;
    log.say(&format!("read {} bytes back", raw.len()));

    let (status, body) = parse_reply(&raw)?;
    report.http_status = Some(status);
    report.body_preview = Some(preview(body));
    log.say(&format!("HTTP {status} through the mesh"));
    Ok(())
}

/// Await `future`, or report which step hung.
///
/// `n0_future::time` rather than `tokio::time`: tokio's timer is not available on
/// wasm32-unknown-unknown, and n0-future is already in the graph as iroh's own answer to that.
async fn with_timeout<F: std::future::Future>(step: &str, future: F) -> Result<F::Output, String> {
    n0_future::time::timeout(STEP_TIMEOUT, future)
        .await
        .map_err(|_| format!("{step} timed out after {STEP_TIMEOUT:?}"))
}

/// The request head, with the node's Basic credential attached.
///
/// `Host: 127.0.0.1` and `Connection: close` for the same reasons the iOS viewer uses them
/// (`adi-mesh-ffi/src/viewer/catalog.rs`): the node never routes on `Host`, and closing is what
/// lets the reply be read to end-of-stream instead of parsed for a length a chunked answer would
/// not carry.
fn head(path: &str, credential: (&str, &str)) -> Vec<u8> {
    let (username, password) = credential;
    let authorization = B64.encode(format!("{username}:{password}"));
    format!(
        "GET {path} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Authorization: Basic {authorization}\r\n\
         Accept: */*\r\n\
         Connection: close\r\n\
         \r\n"
    )
    .into_bytes()
}

/// Split a raw HTTP response into its status code and its body.
fn parse_reply(raw: &[u8]) -> Result<(u16, &[u8]), String> {
    if raw.is_empty() {
        return Err("the node closed the stream without answering".into());
    }
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("the node's answer was not a complete HTTP response")?;
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or("the node's answer carried no status code")?;
    Ok((status, &raw[head_end + 4..]))
}

/// The 32 bytes a 64-character hex string names, or `None` if it is not one.
fn hex32(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (byte, pair) in out.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

/// The first line or so of a body, for the page and the report.
fn preview(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    match text.char_indices().nth(300) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}
