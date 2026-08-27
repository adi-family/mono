// The page half of the bridge: the plumbing between a browser API and the iroh endpoint that
// lives in this tab's wasm.
//
// Two directions arrive here and neither can be answered where it is raised:
//
// * The **service worker** intercepts a panel's `fetch` but cannot dial anything — a worker has no
//   endpoint, and one started and stopped at the browser's discretion is the wrong place to keep a
//   QUIC session. So it asks this tab over a `MessageChannel`, and this tab answers in chunks.
// * A **panel iframe** calls `new WebSocket()`, which no worker ever sees. Its shim asks its parent
//   — this tab — over `postMessage`, and gets a port to speak frames on.
//
// Everything below is transport. The mesh work is Rust (`src/bridge.rs`); this file only moves
// bytes between a port and a handler, and it exists in JavaScript because both APIs are ports and
// events rather than promises.

/**
 * Wire the service worker and the panel iframes to `handlers`.
 *
 * @param {{fetch: Function, socket: Function}} handlers — from `src/bridge.rs`.
 */
export function installBridge(handlers) {
  serviceWorkerBridge(handlers);
  socketBridge(handlers);
}

// --- the service worker --------------------------------------------------------------------

function serviceWorkerBridge(handlers) {
  if (!navigator.serviceWorker) return;

  const announce = () => {
    const worker = navigator.serviceWorker.controller;
    if (!worker) return;
    const channel = new MessageChannel();
    channel.port1.onmessage = (event) => onWorkerMessage(handlers, channel.port1, event.data || {});
    worker.postMessage({ type: "host" }, [channel.port2]);
  };

  // Both, and in this order. On a first-ever load there is no controller until the worker
  // activates and claims; on every load after that there is one immediately and the event never
  // fires again.
  announce();
  navigator.serviceWorker.addEventListener("controllerchange", announce);
}

// The sinks of requests currently in flight, so a `cancel` can reach the right one.
const inFlight = new Map();

function onWorkerMessage(handlers, port, message) {
  if (message.type === "fetch") {
    const sink = {
      cancelled: false,
      head: (status, statusText, headers) =>
        port.postMessage({ type: "head", id: message.id, status, statusText, headers: JSON.parse(headers) }),
      // Transferred, not copied: a panel's wasm bundle is megabytes and this is a phone.
      chunk: (bytes) => port.postMessage({ type: "chunk", id: message.id, bytes }, [bytes.buffer]),
      end: () => {
        inFlight.delete(message.id);
        port.postMessage({ type: "end", id: message.id });
      },
      fail: (text) => {
        inFlight.delete(message.id);
        // Also to the console: the worker turns this into a 503 page inside an iframe, which is
        // the one place a reader is least likely to look for it.
        console.error("adi mesh:", message.method, message.path, "->", text);
        port.postMessage({ type: "error", id: message.id, message: text });
      },
    };
    inFlight.set(message.id, sink);
    handlers.fetch(
      message.node,
      message.method,
      message.path,
      JSON.stringify(message.headers || {}),
      message.body || undefined,
      sink,
    );
  } else if (message.type === "cancel") {
    const sink = inFlight.get(message.id);
    if (sink) sink.cancelled = true;
    inFlight.delete(message.id);
  }
}

// --- a panel's websocket ---------------------------------------------------------------------

function socketBridge(handlers) {
  window.addEventListener("message", (event) => {
    // Same-origin only. The panels are iframes on this origin; nothing else has any business
    // asking this tab to open a socket to a machine of ours.
    if (event.origin !== location.origin) return;
    const message = event.data || {};
    if (message.type !== "adi-ws-open" || !event.ports[0]) return;

    const port = event.ports[0];
    const sink = {
      open: (protocol) => port.postMessage({ type: "open", protocol }),
      message: (data) => port.postMessage({ type: "message", data }),
      closed: (code, reason, clean) => port.postMessage({ type: "close", code, reason, clean }),
      failed: (text) => port.postMessage({ type: "error", message: text }),
    };
    let socket;
    try {
      socket = handlers.socket(message.node, message.path, sink);
    } catch (e) {
      console.error("adi mesh: socket handler threw", e);
      sink.failed(String(e));
      return;
    }
    port.onmessage = (frame) => {
      const outbound = frame.data || {};
      if (outbound.type === "send") socket.send(outbound.data);
      else if (outbound.type === "close") socket.close();
    };
  });
}
