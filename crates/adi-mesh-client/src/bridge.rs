//! Answering the browser: the service worker's intercepted `fetch`, and a panel's `new WebSocket`.
//!
//! Both arrive here as calls from `js/bridge.js`, and both are answered from the one
//! [`Mesh`](crate::mesh::Mesh) this tab holds. The division of labour is worth stating, because it
//! is the whole architecture in three sentences:
//!
//! * The **service worker** decides *which node* a request belongs to and turns the answer back
//!   into a `Response`. It holds no endpoint — a worker the browser stops and starts at will is the
//!   wrong owner for a QUIC session, and a panel whose connection died on an idle timer would be a
//!   panel that breaks after lunch.
//! * **This module** does the dialling, the HTTP and the RFC 6455, and streams the result back a
//!   chunk at a time.
//! * The **page** owns both, and it is the thing that must stay open. Close the tab and every panel
//!   in it goes with it, which is honest: there is no node here, only a reader.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use js_sys::{Object, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::http::{Body, Head, Request};
use crate::mesh::{Mesh, Writer};
use crate::store::NodeRecord;
use crate::ws::{self, Opcode};

/// The one service a paired browser may reach: the node's own control panel.
///
/// `docs/fleet.md` §8 makes `http:app` the default grant, and ADI-13 decided v1 renders that and
/// nothing else — a dashboard on this origin would share storage with the key and the passwords
/// that open every node in the list. Arbitrary services want an origin each, which wants a wildcard
/// domain, which is not v1.
pub const PANEL_SERVICE: &str = "app";

#[wasm_bindgen(module = "/js/bridge.js")]
extern "C" {
    #[wasm_bindgen(js_name = installBridge)]
    fn install_bridge(handlers: &JsValue);
}

#[wasm_bindgen]
extern "C" {
    /// Where a mesh response goes: the worker's side of one intercepted `fetch`.
    pub type Sink;

    #[wasm_bindgen(method)]
    fn head(this: &Sink, status: u16, status_text: String, headers: String);
    #[wasm_bindgen(method)]
    fn chunk(this: &Sink, bytes: Vec<u8>);
    #[wasm_bindgen(method)]
    fn end(this: &Sink);
    #[wasm_bindgen(method)]
    fn fail(this: &Sink, message: String);
    /// Set by the worker when the reader navigated away. Polled between chunks rather than
    /// signalled, because the alternative is a second channel for a boolean.
    #[wasm_bindgen(method, getter)]
    fn cancelled(this: &Sink) -> bool;

    /// Where a mesh websocket's events go: the panel shim's side of one `new WebSocket()`.
    pub type SocketSink;

    #[wasm_bindgen(method)]
    fn open(this: &SocketSink, protocol: String);
    #[wasm_bindgen(method)]
    fn message(this: &SocketSink, data: String);
    #[wasm_bindgen(method)]
    fn closed(this: &SocketSink, code: u16, reason: String, clean: bool);
    #[wasm_bindgen(method)]
    fn failed(this: &SocketSink, message: String);
}

/// Everything the bridge needs: the endpoint, and what this browser knows about each node.
///
/// A thread-local rather than an argument, because the callers are browser events — a worker
/// message, a `postMessage` from an iframe — and there is nowhere for them to have been handed it.
/// Single-threaded, so a `RefCell` is the whole synchronisation story.
struct State {
    mesh: Rc<Mesh>,
    nodes: Vec<NodeRecord>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Hand the bridge the endpoint, and wire it to the browser. Called once, at start-up.
pub fn install(mesh: Rc<Mesh>, nodes: Vec<NodeRecord>) {
    STATE.with(|state| *state.borrow_mut() = Some(State { mesh, nodes }));

    let handlers = Object::new();
    let fetch = Closure::<dyn Fn(String, String, String, String, Option<Vec<u8>>, Sink)>::new(
        |node: String,
         method: String,
         path: String,
         headers: String,
         body: Option<Vec<u8>>,
         sink: Sink| {
            spawn_local(async move {
                if let Err(e) = serve(&node, &method, &path, &headers, body, &sink).await {
                    sink.fail(e);
                }
            });
        },
    );
    let socket = Closure::<dyn Fn(String, String, SocketSink) -> Socket>::new(
        |node: String, path: String, sink: SocketSink| open_socket(node, path, sink),
    );
    let _ = Reflect::set(&handlers, &JsValue::from_str("fetch"), fetch.as_ref());
    let _ = Reflect::set(&handlers, &JsValue::from_str("socket"), socket.as_ref());
    // The handlers object now holds the only reference, and it lives as long as the page.
    fetch.forget();
    socket.forget();

    install_bridge(&handlers);
}

/// Replace what the bridge knows about the node list — after a pairing, a rename or a removal.
pub fn set_nodes(nodes: Vec<NodeRecord>) {
    STATE.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.nodes = nodes;
        }
    });
}

/// The record for `petname`, if this browser has one.
fn node_named(petname: &str) -> Option<NodeRecord> {
    STATE.with(|state| {
        state
            .borrow()
            .as_ref()?
            .nodes
            .iter()
            .find(|n| n.petname == petname)
            .cloned()
    })
}

/// The endpoint this tab holds, once [`install`] has been called.
///
/// Public because the shell needs it to spend an invite, and there is exactly one — a second
/// endpoint would be a second identity, and pairing the wrong one is a mistake with no error
/// message attached to it.
#[must_use]
pub fn endpoint() -> Option<Rc<Mesh>> {
    STATE.with(|state| Some(Rc::clone(&state.borrow().as_ref()?.mesh)))
}

// ---------------------------------------------------------------------------------------
// One intercepted fetch
// ---------------------------------------------------------------------------------------

/// Fetch `path` from `node`'s panel and stream the answer into `sink`.
async fn serve(
    node: &str,
    method: &str,
    path: &str,
    headers: &str,
    body: Option<Vec<u8>>,
    sink: &Sink,
) -> Result<(), String> {
    let record = node_named(node).ok_or_else(|| {
        format!("this browser is not paired with a node called {node:?} — pair it again")
    })?;
    let mesh = endpoint().ok_or("the mesh endpoint is not up in this tab")?;

    let mut stream = mesh.open(&record.addr()?, PANEL_SERVICE).await?;
    let mut request = Request {
        method: method.to_string(),
        target: path.to_string(),
        headers: parse_headers(headers),
        body: body.unwrap_or_default(),
    }
    .with_basic_auth(&record.username, &record.password);
    // Keep-alive is the point of pooling, and a request that said `close` would cost a fresh
    // bi-stream — and on a carved host a fresh TCP connection on the node — for every asset.
    request = request.with("Connection", "keep-alive");
    stream.write(&request.encode()).await?;

    let head = Head::parse(&stream.read_head().await?)?;
    sink.head(
        head.status,
        head.reason.clone(),
        headers_json(&head.headers),
    );

    let mut reader = Body::new(&head);
    while !reader.is_done() {
        if sink.cancelled() {
            // The reader closed the panel or navigated away. Dropping the stream resets it, which
            // is what stops a node writing into an event feed nobody is reading.
            return Ok(());
        }
        let chunk = reader.next(stream.reader()).await?;
        if chunk.is_empty() {
            break;
        }
        sink.chunk(chunk);
    }
    sink.end();
    Ok(())
}

/// The headers the worker forwarded, as pairs. A malformed value is dropped rather than fatal:
/// this is a browser's own header list, and one entry we cannot read is not worth the request.
fn parse_headers(json: &str) -> Vec<(String, String)> {
    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(json)
        .map(|map| {
            map.into_iter()
                .filter_map(|(name, value)| Some((name, value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// The response's headers as a JSON object, which is what the worker builds a `Headers` from.
///
/// A repeated header keeps its **last** value. HTTP allows repeats and `Set-Cookie` is the one
/// that matters — but a node's cookie stored against this origin would be a cookie shared with
/// every other node opened here, so losing all but one is the safe direction to lose in.
fn headers_json(headers: &[(String, String)]) -> String {
    let map: serde_json::Map<String, serde_json::Value> = headers
        .iter()
        .map(|(name, value)| (name.clone(), serde_json::Value::String(value.clone())))
        .collect();
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
}

// ---------------------------------------------------------------------------------------
// One panel websocket
// ---------------------------------------------------------------------------------------

/// A live mesh websocket, as the panel shim holds it.
#[wasm_bindgen]
#[derive(Debug)]
pub struct Socket {
    outbox: Rc<RefCell<Outbox>>,
}

/// The write half plus a queue in front of it.
///
/// A queue and not a direct write, because `send()` is called from an event handler that cannot
/// await, and two `write_all`s in flight on one QUIC stream would interleave two websocket frames
/// into one unreadable run of bytes.
#[derive(Debug)]
struct Outbox {
    queue: VecDeque<Vec<u8>>,
    writer: Option<Writer>,
    pumping: bool,
}

#[wasm_bindgen]
impl Socket {
    /// Send one text message.
    ///
    /// By value because wasm-bindgen marshals a JS string into an owned `String` either way; a
    /// `&str` here would only move the allocation to the generated glue.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the wasm-bindgen ABI owns the string"
    )]
    pub fn send(&self, text: String) {
        self.outbox
            .borrow_mut()
            .queue
            .push_back(ws::encode(Opcode::Text, text.as_bytes()));
        pump(&self.outbox);
    }

    /// Send a close frame. The read loop ends when the node answers or the stream does.
    pub fn close(&self) {
        self.outbox
            .borrow_mut()
            .queue
            .push_back(ws::encode(Opcode::Close, &1000u16.to_be_bytes()));
        pump(&self.outbox);
    }
}

/// Drain the outbox, one write at a time, until it is empty.
fn pump(outbox: &Rc<RefCell<Outbox>>) {
    if outbox.borrow().pumping {
        return;
    }
    outbox.borrow_mut().pumping = true;
    let outbox = Rc::clone(outbox);
    spawn_local(async move {
        loop {
            // Every borrow is released before the await below, which is what makes a `RefCell`
            // safe here at all.
            //
            // The queue is checked *before* the writer is taken, and not in one pattern with it.
            // Taking it first and then finding nothing to send drops the `Writer` on the way out
            // of the failed match — which resets the QUIC stream, ends the node's splice, and
            // closes a websocket that had just finished its handshake. That is exactly what
            // happened here, and the symptom was a panel whose socket opened and immediately went
            // quiet with nothing logged anywhere.
            let Some(bytes) = outbox.borrow_mut().queue.pop_front() else {
                outbox.borrow_mut().pumping = false;
                return;
            };
            let Some(mut writer) = outbox.borrow_mut().writer.take() else {
                outbox.borrow_mut().pumping = false;
                return;
            };
            let wrote = writer.write(&bytes).await;
            let mut held = outbox.borrow_mut();
            held.writer = Some(writer);
            if wrote.is_err() {
                held.pumping = false;
                return;
            }
        }
    });
}

/// Open a websocket to `path` on `node`'s panel, and pump its frames into `sink`.
fn open_socket(node: String, path: String, sink: SocketSink) -> Socket {
    let outbox = Rc::new(RefCell::new(Outbox {
        queue: VecDeque::new(),
        writer: None,
        pumping: false,
    }));
    let socket = Socket {
        outbox: Rc::clone(&outbox),
    };

    spawn_local(async move {
        match connect(&node, &path).await {
            Ok(stream) => {
                let (writer, mut reader) = stream.split();
                outbox.borrow_mut().writer = Some(writer);
                // Anything the shim queued before the handshake finished goes now.
                pump(&outbox);
                sink.open(String::new());

                loop {
                    match ws::read_message(&mut reader).await {
                        Ok(Some(message)) => match message.opcode {
                            Opcode::Text | Opcode::Binary => {
                                sink.message(
                                    String::from_utf8_lossy(&message.payload).into_owned(),
                                );
                            }
                            Opcode::Ping => {
                                outbox
                                    .borrow_mut()
                                    .queue
                                    .push_back(ws::encode(Opcode::Pong, &message.payload));
                                pump(&outbox);
                            }
                            Opcode::Pong => {}
                            Opcode::Close => {
                                let code = match message.payload.as_slice() {
                                    [hi, lo, ..] => u16::from_be_bytes([*hi, *lo]),
                                    _ => 1005,
                                };
                                sink.closed(code, String::new(), true);
                                return;
                            }
                        },
                        // End of stream with no close frame: the node or the relay went away. 1006
                        // is what a browser reports for exactly this, so the panel's reconnect
                        // logic sees what it would see on a local socket.
                        Ok(None) => {
                            sink.closed(1006, String::new(), false);
                            return;
                        }
                        Err(e) => {
                            sink.failed(e);
                            return;
                        }
                    }
                }
            }
            Err(e) => sink.failed(e),
        }
    });

    socket
}

/// Dial, admit, and complete the RFC 6455 handshake.
async fn connect(node: &str, path: &str) -> Result<crate::mesh::Stream, String> {
    let record = node_named(node)
        .ok_or_else(|| format!("this browser is not paired with a node called {node:?}"))?;
    let mesh = endpoint().ok_or("the mesh endpoint is not up in this tab")?;
    let mut stream = mesh.open(&record.addr()?, PANEL_SERVICE).await?;
    let request = Request::get(path)
        .with_basic_auth(&record.username, &record.password)
        // The panel's `/api/ws` has no guard but this one: a websocket handshake is exempt from
        // CORS and `new WebSocket()` cannot set a header, so `Origin` is the only thing that can
        // ever gate it (`adi-app/src/origin.rs`). It has to name the `Host` this request carries.
        .with("Origin", "http://127.0.0.1");
    ws::handshake(&mut stream, request).await?;
    Ok(stream)
}
