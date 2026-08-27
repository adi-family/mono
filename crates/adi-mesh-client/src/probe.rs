//! The measurement this client is allowed to exist because of: **can a long-lived stream cross
//! `adi/mesh/http/1` from a browser tab?**
//!
//! The spike that proved a tab can be an iroh peer (`spikes/mesh-browser`) tested exactly one
//! request and one response. A control panel needs more than that — its live channel is a
//! websocket, and any dashboard worth opening pushes — and nothing in the mesh had ever carried a
//! response that does not end. If it could not, this client could not serve the panel and the
//! design changed; so it is measured here, first, against a real `adi-mesh` daemon, before any of
//! the UI exists.
//!
//! It stays in the shipped crate rather than living in `spikes/` for one reason: it is a
//! regression test for somebody else's code. The property being measured is that
//! `gateway::negotiate` exempts an upgrade from `Connection: close` and that `tunnel::splice`
//! pumps rather than buffers. Both are one careless edit away from becoming false, in a crate that
//! has no browser in its own test suite. `harness/run.sh` runs this against a scratch node.
//!
//! Every case is a real dial to a real node over a real relay. There are no fakes here — a fake
//! would answer the question about the fake.

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::http::{Body, Head, Request};
use crate::mesh::Mesh;
use crate::ws::{self, Opcode};
use crate::{invite, now_ms};

/// What the harness hands the page.
#[derive(Debug, Deserialize)]
pub struct Options {
    /// 64 hex characters: the identity the node has already been told to authorize.
    pub secret: String,
    /// The node, as an `adimesh:` ticket or a bare endpoint id.
    pub node: String,
    /// The relay the node calls home, when `node` is a bare id.
    #[serde(default)]
    pub relay: String,
    /// The service to reach on the node.
    pub service: String,
    /// The Basic credential for it.
    pub username: String,
    /// Its password.
    pub password: String,
    /// The path that answers with an event stream.
    pub sse_path: String,
    /// The path that answers a websocket upgrade.
    pub ws_path: String,
    /// How many events to wait for before calling the stream proved.
    pub sse_events: usize,
}

/// One case's verdict.
#[derive(Debug, Serialize)]
pub struct Case {
    /// What was being asked.
    pub name: String,
    /// Whether it answered as it must.
    pub ok: bool,
    /// What went wrong, when something did.
    pub error: Option<String>,
    /// The finding in one sentence, for a human reading the report.
    pub detail: String,
    /// Every observation with the millisecond it was made at. A stream that was buffered and one
    /// that was pumped differ *only* here — both deliver the same bytes.
    pub marks: Vec<Mark>,
}

/// One observation, stamped.
#[derive(Debug, Serialize)]
pub struct Mark {
    /// Milliseconds since the probe started.
    pub at_ms: u32,
    /// What was seen.
    pub what: String,
}

/// The whole run.
#[derive(Debug, Serialize)]
pub struct Report {
    /// Whether every case passed.
    pub ok: bool,
    /// This tab's own key.
    pub browser_key: Option<String>,
    /// A fatal error before any case could run.
    pub error: Option<String>,
    /// The cases, in order.
    pub cases: Vec<Case>,
}

/// A clock that stamps observations from one origin.
struct Clock {
    started: f64,
    marks: Vec<Mark>,
}

impl Clock {
    fn new() -> Self {
        Self {
            started: now_ms(),
            marks: Vec::new(),
        }
    }

    fn mark(&mut self, what: impl Into<String>) {
        let what = what.into();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a probe that ran for 49 days would have other problems"
        )]
        let at_ms = (now_ms() - self.started).max(0.0) as u32;
        crate::log(&format!("[{at_ms:>6}ms] {what}"));
        self.marks.push(Mark { at_ms, what });
    }

    /// The span between the first and last observation — the number that separates a stream that
    /// was pumped from one that was buffered and delivered at the end.
    fn spread(&self) -> u32 {
        match (self.marks.first(), self.marks.last()) {
            (Some(first), Some(last)) => last.at_ms.saturating_sub(first.at_ms),
            _ => 0,
        }
    }
}

/// Run every case and return the report as JSON.
///
/// Never returns `Err`: a failure is a result, and a probe that threw it away as a JS exception
/// would lose the marks that say *where* it failed.
#[wasm_bindgen]
pub async fn probe(options: String) -> String {
    crate::init_logging();
    let mut report = Report {
        ok: false,
        browser_key: None,
        error: None,
        cases: Vec::new(),
    };

    let options: Options = match serde_json::from_str(&options) {
        Ok(options) => options,
        Err(e) => {
            report.error = Some(format!("the probe options do not parse: {e}"));
            return json(&report);
        }
    };

    let secret = match invite::secret_from_hex(&options.secret) {
        Ok(secret) => secret,
        Err(e) => {
            report.error = Some(e);
            return json(&report);
        }
    };
    let mesh = match Mesh::bind(secret).await {
        Ok(mesh) => mesh,
        Err(e) => {
            report.error = Some(e);
            return json(&report);
        }
    };
    report.browser_key = Some(mesh.id().to_string());
    crate::log(&format!("this tab's key is {}", mesh.id()));

    let addr = match invite::addr_from(&options.node, &options.relay) {
        Ok(addr) => addr,
        Err(e) => {
            report.error = Some(e);
            return json(&report);
        }
    };

    report.cases.push(one_request(&mesh, &addr, &options).await);
    report
        .cases
        .push(event_stream(&mesh, &addr, &options).await);
    report.cases.push(websocket(&mesh, &addr, &options).await);
    report.ok = report.cases.iter().all(|case| case.ok);
    json(&report)
}

/// The spike's case, re-run: the floor everything else stands on.
async fn one_request(mesh: &Mesh, addr: &iroh::EndpointAddr, options: &Options) -> Case {
    let mut clock = Clock::new();
    let result = async {
        let mut stream = mesh.open(addr, &options.service).await?;
        clock.mark("stream admitted");
        let request = Request::get("/")
            .with_basic_auth(&options.username, &options.password)
            .with("Connection", "close");
        stream.write(&request.encode()).await?;
        stream.finish();
        clock.mark("sent GET /");

        let head = Head::parse(&stream.read_head().await?)?;
        clock.mark(format!("HTTP {} through the mesh", head.status));
        let mut body = Body::new(&head);
        let mut bytes = 0;
        while !body.is_done() {
            bytes += body.next(stream.reader()).await?.len();
        }
        clock.mark(format!("read {bytes} bytes of body"));
        Ok::<_, String>(format!("HTTP {} and {bytes} bytes of body", head.status))
    }
    .await;
    finish("one request and one response", result, clock)
}

/// **The case this whole probe exists for.** An event stream is a response with no length that
/// never ends: if the node buffered it, every event would arrive together at the close.
///
/// The verdict is the *spread* of arrival times, not the bytes. A buffered stream delivers the
/// same bytes — it just delivers them all at once, at the end.
async fn event_stream(mesh: &Mesh, addr: &iroh::EndpointAddr, options: &Options) -> Case {
    let mut clock = Clock::new();
    let wanted = options.sse_events.max(2);
    let result = async {
        let mut stream = mesh.open(addr, &options.service).await?;
        clock.mark("stream admitted");
        // No `Connection: close` and no `finish()`: an event stream is read while the request half
        // is still open, and finishing would splice a FIN through to the node's local service.
        let request = Request::get(&options.sse_path)
            .with_basic_auth(&options.username, &options.password)
            .with("Accept", "text/event-stream");
        stream.write(&request.encode()).await?;
        clock.mark(format!("sent GET {}", options.sse_path));

        let head = Head::parse(&stream.read_head().await?)?;
        clock.mark(format!(
            "HTTP {} ({})",
            head.status,
            head.get("content-type").unwrap_or("no content-type")
        ));
        if head.status != 200 {
            return Err(format!("the event stream answered {}", head.status));
        }

        let mut body = Body::new(&head);
        let mut pending = String::new();
        let mut seen = 0;
        while seen < wanted {
            let chunk = body.next(stream.reader()).await?;
            if chunk.is_empty() {
                return Err(format!(
                    "the stream ended after {seen} of {wanted} events — the node closed it, or \
                     the service did"
                ));
            }
            pending.push_str(&String::from_utf8_lossy(&chunk));
            // One mark per `data:` line, at the moment its bytes reached this tab.
            while let Some(at) = pending.find('\n') {
                let line: String = pending.drain(..=at).collect();
                if let Some(data) = line.trim_end().strip_prefix("data:") {
                    seen += 1;
                    clock.mark(format!("event {seen}: {}", data.trim()));
                }
            }
        }
        // Dropping the stream is what stops the feed; the node's splice sees the reset.
        drop(stream);
        Ok::<_, String>(seen)
    }
    .await;

    let spread = clock.spread();
    match result {
        Ok(seen) => {
            // The service emits one event a second, so `seen` events cannot legitimately arrive in
            // less time than that. Arriving together is exactly the buffering this case is looking
            // for, and it would otherwise read as a pass.
            let floor = u32::try_from(seen.saturating_sub(1)).unwrap_or(u32::MAX) * 500;
            let streamed = spread >= floor;
            Case {
                name: "a long-lived event stream".into(),
                ok: streamed,
                error: (!streamed).then(|| {
                    format!("{seen} events arrived within {spread} ms — they were buffered, not streamed")
                }),
                detail: format!("{seen} events spread over {spread} ms"),
                marks: clock.marks,
            }
        }
        Err(e) => finish_err("a long-lived event stream", e, clock),
    }
}

/// The other long-lived shape, and the one the control panel actually uses: a websocket, spoken by
/// this tab over the same splice.
async fn websocket(mesh: &Mesh, addr: &iroh::EndpointAddr, options: &Options) -> Case {
    let mut clock = Clock::new();
    let result = async {
        let mut stream = mesh.open(addr, &options.service).await?;
        clock.mark("stream admitted");
        let request = Request::get(&options.ws_path)
            .with_basic_auth(&options.username, &options.password)
            // What a browser would send, and what the panel checks instead of a token
            // (`adi-app/src/origin.rs`): a handshake that carries no `Origin` passes, and one that
            // carries a foreign one is refused. Naming the `Host` this request carries is what
            // makes it same-origin from the node's point of view.
            .with("Origin", "http://127.0.0.1");
        let head = ws::handshake(&mut stream, request).await?;
        clock.mark(format!(
            "101, sec-websocket-accept verified ({})",
            head.get("sec-websocket-protocol")
                .unwrap_or("no subprotocol")
        ));

        // A round trip: the echo cannot arrive before the send, so this measures the mesh's own
        // latency for a message rather than for a connection.
        for n in 1..=3 {
            let sent = format!("ping-{n}");
            stream
                .write(&ws::encode(Opcode::Text, sent.as_bytes()))
                .await?;
            clock.mark(format!("sent {sent}"));
            let message = ws::read_message(stream.reader())
                .await?
                .ok_or("the socket closed instead of answering")?;
            let got = String::from_utf8_lossy(&message.payload).to_string();
            clock.mark(format!("received {got:?} ({:?})", message.opcode));
            if message.opcode != Opcode::Text || got != sent {
                return Err(format!("the echo answered {got:?} to {sent:?}"));
            }
        }

        // And the direction that matters for a live channel: the *server* speaking first, with
        // nothing outstanding from this side.
        let pushed = ws::read_message(stream.reader())
            .await?
            .ok_or("the socket closed instead of pushing")?;
        clock.mark(format!(
            "server pushed {:?} unprompted",
            String::from_utf8_lossy(&pushed.payload)
        ));

        stream
            .write(&ws::encode(Opcode::Close, &1000u16.to_be_bytes()))
            .await?;
        clock.mark("sent close");
        Ok::<_, String>("echo and unprompted push both crossed the splice".to_string())
    }
    .await;
    finish("a websocket, spoken by the tab", result, clock)
}

fn finish(name: &str, result: Result<String, String>, clock: Clock) -> Case {
    match result {
        Ok(detail) => Case {
            name: name.into(),
            ok: true,
            error: None,
            detail,
            marks: clock.marks,
        },
        Err(e) => finish_err(name, e, clock),
    }
}

fn finish_err(name: &str, error: String, clock: Clock) -> Case {
    crate::log(&format!("FAILED: {name}: {error}"));
    Case {
        name: name.into(),
        ok: false,
        error: Some(error),
        detail: String::new(),
        marks: clock.marks,
    }
}

fn json(report: &Report) -> String {
    serde_json::to_string_pretty(report)
        .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"report did not serialise: {e}\"}}"))
}
