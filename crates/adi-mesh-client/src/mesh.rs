//! The transport: an [`iroh::Endpoint`] in a browser tab, and one HTTP connection per bi-stream.
//!
//! This is the half of the client that is the same work the Mac front door and the iOS viewer do
//! — dial a node by key, write the [`protocol`] frame, read the status byte, then speak HTTP —
//! with two differences that come from being a tab and are worth stating once:
//!
//! * **It dials and never accepts.** No `alpns()` is set on the endpoint, so nothing can open a
//!   stream to this tab. Every ALPN we speak is one we opened.
//! * **Everything is relayed, for the whole session.** A browser has no UDP socket, so there is no
//!   direct path to hole-punch and no fallback if the relay is unreachable (`docs/fleet.md` §9).
//!   That makes the relay a hard precondition rather than a latency question, which is why
//!   [`Mesh::bind`] pins one instead of taking iroh's preset.
//!
//! Connections are **pooled per node**, for the reason `gateway.rs` pools them: a fresh iroh
//! connection per request would put a QUIC handshake — over a relay, so a round trip to Madrid —
//! in front of every image on the page.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey};

use crate::protocol::{self, HttpStatus};

/// How long any one step of opening a stream may take before it is reported as a hang.
///
/// Generous, because the first step of a cold session also pays for the relay's TLS and websocket
/// handshake. It bounds *setup* only — never a stream that is already carrying bytes, which is
/// exactly what a long-lived one looks like and what a timeout here would kill.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(20);

/// The relay this client calls home when nothing else is said.
///
/// Pinned here rather than read from iroh's preset, and it is `adi_mesh::relay::DEFAULT_RELAYS`
/// said again on purpose: that constant lives in a crate this one cannot depend on (it reaches
/// `adi_config` and the local store) and the value is load-bearing in a way a default is not — a
/// browser that lands on n0's public relay cannot dial *anything*, because that relay answers the
/// websocket upgrade without echoing `sec-websocket-protocol` and RFC 6455 §4.1 makes the browser
/// fail the connection (measured 2026-08-24, `spikes/mesh-browser`).
pub const HOME_RELAY: &str = "https://mad.mono-relay.withadi.dev";

/// A dial that failed, with enough detail for the reader to know whose fault it is.
pub type Result<T> = std::result::Result<T, String>;

/// The endpoint, and the connection it is holding open to each node.
#[derive(Debug)]
pub struct Mesh {
    endpoint: Endpoint,
    /// One live [`Connection`] per node. `RefCell` and not a lock because wasm is single-threaded
    /// and every borrow here is released before an `.await`.
    conns: RefCell<HashMap<EndpointId, Rc<Connection>>>,
}

impl Mesh {
    /// Bind an endpoint on `secret`, homed on [`HOME_RELAY`].
    ///
    /// **Does not wait for the home relay session**, and that is not an optimisation. `online()`
    /// waits for the relay through which this tab could be *reached*, and nothing ever dials a
    /// browser: in the spike it timed out at 8 s in one case and the dial worked anyway. A node is
    /// reached through *its* relay, which travels in the [`EndpointAddr`].
    ///
    /// # Errors
    /// If the relay URL does not parse or the endpoint cannot bind.
    pub async fn bind(secret: SecretKey) -> Result<Self> {
        let home: RelayUrl = HOME_RELAY
            .parse()
            .map_err(|e| format!("the home relay {HOME_RELAY} does not parse: {e}"))?;
        // `presets::Minimal` and not `presets::N0`: N0 adds a pkarr *publisher*, which announces
        // this tab's address into n0's DNS. A client that is dialled by nobody publishes for
        // nobody, so it is a leak with no upside — and with the node's relay carried in its
        // address, no lookup is needed either.
        //
        // No `.alpns(…)`: this endpoint dials and never accepts.
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(secret)
            .relay_mode(RelayMode::Custom(RelayMap::from_iter([home])))
            .bind()
            .await
            .map_err(|e| format!("binding the endpoint failed: {e}"))?;
        Ok(Self {
            endpoint,
            conns: RefCell::new(HashMap::new()),
        })
    }

    /// This client's own key — what a node's operator authorizes.
    #[must_use]
    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// Forget the pooled connection to `node`, so the next call dials afresh.
    pub fn drop_connection(&self, node: EndpointId) {
        self.conns.borrow_mut().remove(&node);
    }

    /// A live connection to `addr` on `alpn`, dialling only if the pooled one is gone.
    ///
    /// # Errors
    /// If the dial fails or times out.
    pub async fn connect(&self, addr: &EndpointAddr, alpn: &[u8]) -> Result<Rc<Connection>> {
        let id = addr.id;
        // A pooled connection whose peer has gone away reports a `close_reason`; reusing it would
        // fail every stream opened on it until something noticed. Checking here is what makes a
        // node that slept and came back reachable again without a reload.
        if let Some(conn) = self.conns.borrow().get(&id)
            && conn.close_reason().is_none()
        {
            return Ok(Rc::clone(conn));
        }
        self.conns.borrow_mut().remove(&id);

        let conn = with_timeout(
            "dialling the node",
            self.endpoint.connect(addr.clone(), alpn),
        )
        .await?
        .map_err(|e| format!("dialling the node failed: {e}"))?;
        let conn = Rc::new(conn);
        // Only the gateway ALPN is worth pooling: a join is one stream, once, and holding it open
        // afterwards would keep a connection alive for a handshake that is finished.
        if alpn == protocol::HTTP_ALPN {
            self.conns.borrow_mut().insert(id, Rc::clone(&conn));
        }
        Ok(conn)
    }

    /// Open one HTTP connection to `service` on `addr`: a bi-stream, the gateway frame, and the
    /// node's status byte read back.
    ///
    /// The returned [`Stream`] is at the point where raw HTTP bytes flow, in both directions, for
    /// as long as either side keeps it open. Nothing here caps it or reads it to end — see
    /// [`Stream::read`].
    ///
    /// # Errors
    /// A refusal from the node (as its own sentence), or any transport error.
    pub async fn open(&self, addr: &EndpointAddr, service: &str) -> Result<Stream> {
        let conn = self.connect(addr, protocol::HTTP_ALPN).await?;
        let (mut send, mut recv) = with_timeout("opening a stream", conn.open_bi())
            .await?
            .map_err(|e| format!("opening a stream failed: {e}"))?;

        protocol::write_http_request(&mut send, service)
            .await
            .map_err(|e| format!("writing the service frame failed: {e}"))?;

        let status = with_timeout(
            "reading the mesh status byte",
            protocol::read_http_status(&mut recv),
        )
        .await?
        .map_err(|e| format!("reading the mesh status failed: {e}"))?;
        if status != HttpStatus::Ok {
            // The node's own words. These are the three refusals `docs/fleet.md` §7 defines, and
            // the caller renders them as a local page rather than guessing from a status line it
            // never received.
            return Err(status.reason().to_string());
        }
        Ok(Stream {
            writer: Writer { send },
            reader: Reader {
                recv,
                buffered: Vec::new(),
            },
        })
    }
}

/// One HTTP connection over the mesh: a [`Writer`] and a [`Reader`] on the same bi-stream.
///
/// The two are separable ([`split`](Self::split)) because a websocket needs them apart: its read
/// loop sits in a task waiting for whatever the node says next, while `send()` is called from an
/// event handler that cannot wait for that loop to yield.
#[derive(Debug)]
pub struct Stream {
    writer: Writer,
    reader: Reader,
}

impl Stream {
    /// Write `bytes` to the node.
    ///
    /// # Errors
    /// Any write error on the stream.
    pub async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write(bytes).await
    }

    /// Say the request is complete, so the node's splice shuts its upstream write half.
    ///
    /// Only correct where no more bytes will ever be sent. A keep-alive connection, a websocket
    /// and any request whose response the caller means to answer must **not** finish: the node
    /// splices the FIN straight through to the local service, which then stops reading.
    pub fn finish(&mut self) {
        self.writer.finish();
    }

    /// The read half, for the body reader and the frame reader.
    pub fn reader(&mut self) -> &mut Reader {
        &mut self.reader
    }

    /// Read until the end of an HTTP head — see [`Reader::read_head`].
    ///
    /// # Errors
    /// As [`Reader::read_head`].
    pub async fn read_head(&mut self) -> Result<Vec<u8>> {
        self.reader.read_head().await
    }

    /// Take the two halves apart, so each can live in its own task.
    #[must_use]
    pub fn split(self) -> (Writer, Reader) {
        (self.writer, self.reader)
    }
}

/// The write half of a mesh stream.
#[derive(Debug)]
pub struct Writer {
    send: SendStream,
}

impl Writer {
    /// Write `bytes` to the node.
    ///
    /// # Errors
    /// Any write error on the stream.
    pub async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.send
            .write_all(bytes)
            .await
            .map_err(|e| format!("writing to the node failed: {e}"))
    }

    /// Finish the write half — see [`Stream::finish`] for when that is right.
    pub fn finish(&mut self) {
        let _ = self.send.finish();
    }
}

/// The read half, plus whatever was read past the last caller's boundary.
///
/// The buffer is why this is a type and not a bare `RecvStream`. A response head has to be read in
/// chunks, and the read that finds `\r\n\r\n` will usually have swallowed some of the body — a
/// reader that dropped it would corrupt every chunked response and every websocket frame that
/// arrived in the same packet as the `101`.
#[derive(Debug)]
pub struct Reader {
    recv: RecvStream,
    buffered: Vec<u8>,
}

impl Reader {
    /// Read the next bytes the node sent, waiting if there are none yet.
    ///
    /// Returns an empty vector at end of stream. **Unbounded in time on purpose** — this is the
    /// call an SSE reader and a websocket sit in, and a timeout around it would end exactly the
    /// connections this client exists to hold open.
    ///
    /// # Errors
    /// Any read error on the stream.
    pub async fn read(&mut self) -> Result<Vec<u8>> {
        if !self.buffered.is_empty() {
            return Ok(std::mem::take(&mut self.buffered));
        }
        let mut buf = vec![0u8; 16 * 1024];
        // iroh's own `read` — `None` is end of stream, which is what an empty vector means here.
        let n = self
            .recv
            .read(&mut buf)
            .await
            .map_err(|e| format!("reading from the node failed: {e}"))?
            .unwrap_or(0);
        buf.truncate(n);
        Ok(buf)
    }

    /// Put bytes back, to be handed to the next [`read`](Self::read).
    pub fn unread(&mut self, bytes: &[u8]) {
        let mut rest = bytes.to_vec();
        rest.extend_from_slice(&self.buffered);
        self.buffered = rest;
    }

    /// Read until the end of an HTTP head (`\r\n\r\n`), returning the head and leaving the rest
    /// buffered.
    ///
    /// # Errors
    /// If the stream ends before the head is complete, or the head exceeds [`MAX_HEAD`].
    pub async fn read_head(&mut self) -> Result<Vec<u8>> {
        let mut head = Vec::new();
        loop {
            let chunk = self.read().await?;
            if chunk.is_empty() {
                return Err(if head.is_empty() {
                    "the node closed the stream without answering".into()
                } else {
                    "the node's answer was not a complete HTTP response".into()
                });
            }
            // Search from just before the join, so a `\r\n\r\n` split across two chunks is found.
            let from = head.len().saturating_sub(3);
            head.extend_from_slice(&chunk);
            if let Some(at) = head[from..].windows(4).position(|w| w == b"\r\n\r\n") {
                let end = from + at + 4;
                let rest = head.split_off(end);
                self.unread(&rest);
                return Ok(head);
            }
            if head.len() > MAX_HEAD {
                return Err(format!(
                    "the node's response head passed {MAX_HEAD} bytes without ending"
                ));
            }
        }
    }
}

/// The most of a response head this client will buffer before calling the peer broken. Generous
/// against real heads (a `Set-Cookie` run is measured in hundreds of bytes) and small enough that
/// a peer answering with a body and no head cannot make a phone allocate for it.
const MAX_HEAD: usize = 64 * 1024;

/// Await `future`, or say which step hung.
///
/// `n0_future::time` rather than `tokio::time`: tokio's timer does not exist on
/// wasm32-unknown-unknown, and n0-future is already in the graph as iroh's own answer to that.
///
/// # Errors
/// The step's name and the elapsed limit, when it did not finish in time.
pub async fn with_timeout<F: std::future::Future>(step: &str, future: F) -> Result<F::Output> {
    n0_future::time::timeout(STEP_TIMEOUT, future)
        .await
        .map_err(|_| format!("{step} timed out after {STEP_TIMEOUT:?}"))
}

/// The address to dial for a node: its key, plus the relay it calls home.
///
/// The relay travels with the address rather than being looked up, which is what lets one client
/// served from one domain reach nodes on different relays — and what keeps it working for a node
/// whose relay is not in this tab's own map (proved as case 5 of the spike).
#[must_use]
pub fn addr_of(id: EndpointId, relay: Option<&RelayUrl>) -> EndpointAddr {
    match relay {
        Some(relay) => EndpointAddr::new(id).with_relay_url(relay.clone()),
        None => EndpointAddr::new(id),
    }
}
