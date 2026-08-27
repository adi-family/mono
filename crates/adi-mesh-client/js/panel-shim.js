// Injected as the first thing inside a panel's <head> (see `sw.js`). Two jobs, both of which have
// to happen before any of the panel's own scripts run.
//
// **1. Give the panel back the root.** The iframe is opened at `/n/<petname>/` so the worker can
// learn which node this client belongs to. The panel is a single-page app that reads
// `location.pathname` to route and writes root-absolute paths with `pushState`
// (`adi-webapp/src/routing.rs`), so it must not see that prefix. Rewriting the URL here — before
// the panel boots, and after the worker has recorded the client id — means the panel behaves
// byte for byte as it does at `app.adi`: its router works, its links work, the back button works.
//
// **2. Route `new WebSocket()` through the mesh.** This is the one thing a service worker cannot
// help with: a websocket handshake is not a `fetch`, no `fetch` event ever fires for it, and there
// is no interception point anywhere in the platform. So the constructor is replaced with one that
// asks the shell — the tab that holds the iroh endpoint — to open the socket over the mesh and
// pump its frames back. The panel's live channel (`adi-webapp/src/live.rs`) is the caller that
// matters; without this it falls back to polling, which works but sends every read twice a second.

(() => {
  const script = document.currentScript;
  const node = (script && script.dataset.node) || "";
  if (!node) return;

  // Tell the worker who we are before anything else: `event.source.id` on its side is the only
  // authority on this client's id, and it is what keeps the mapping alive across a worker restart.
  if (navigator.serviceWorker && navigator.serviceWorker.controller) {
    navigator.serviceWorker.controller.postMessage({ type: "claim", node });
  }

  try {
    history.replaceState(null, "", "/");
  } catch {
    // A browser that refuses the rewrite leaves the panel on `/n/<node>/`, where its router lands
    // on its home view and everything else still works. Worth degrading to, not worth failing on.
  }

  // --- the websocket -----------------------------------------------------------------------

  const CONNECTING = 0;
  const OPEN = 1;
  const CLOSING = 2;
  const CLOSED = 3;

  class MeshWebSocket extends EventTarget {
    constructor(url, protocols) {
      super();
      this.url = String(url);
      this.protocol = "";
      this.extensions = "";
      this.binaryType = "blob";
      this.readyState = CONNECTING;
      this.onopen = null;
      this.onmessage = null;
      this.onclose = null;
      this.onerror = null;

      // The panel builds an absolute `ws://<host>/api/ws` from `location`, so only the path is
      // ours to keep — the authority names this origin, which the node has never heard of.
      let path = "/";
      try {
        path = new URL(this.url, location.href).pathname;
      } catch {
        path = this.url.startsWith("/") ? this.url : "/" + this.url;
      }

      const channel = new MessageChannel();
      this.port = channel.port1;
      this.port.onmessage = (event) => this.receive(event.data || {});
      // The shell is the parent window, and `location.origin` rather than `*`: this message names
      // a node and would be worth reading to anybody who could.
      parent.postMessage({ type: "adi-ws-open", node, path, protocols: protocols || null },
        location.origin, [channel.port2]);
    }

    receive(message) {
      if (message.type === "open" && this.readyState === CONNECTING) {
        this.readyState = OPEN;
        this.protocol = message.protocol || "";
        this.fire(new Event("open"), "onopen");
      } else if (message.type === "message") {
        this.fire(new MessageEvent("message", { data: message.data }), "onmessage");
      } else if (message.type === "close") {
        this.finish(message.code || 1000, message.reason || "", message.clean !== false);
      } else if (message.type === "error") {
        this.fire(new Event("error"), "onerror");
        this.finish(1006, message.message || "", false);
      }
    }

    finish(code, reason, clean) {
      if (this.readyState === CLOSED) return;
      this.readyState = CLOSED;
      this.port.onmessage = null;
      this.port.close();
      this.fire(new CloseEvent("close", { code, reason, wasClean: clean }), "onclose");
    }

    /** Both dispatch paths, because the panel uses the `on…` properties and other code may not. */
    fire(event, property) {
      const handler = this[property];
      if (typeof handler === "function") handler.call(this, event);
      this.dispatchEvent(event);
    }

    send(data) {
      if (this.readyState !== OPEN) {
        throw new DOMException("the socket is not open", "InvalidStateError");
      }
      this.port.postMessage({ type: "send", data: String(data) });
    }

    close(code, reason) {
      if (this.readyState === CLOSED || this.readyState === CLOSING) return;
      this.readyState = CLOSING;
      this.port.postMessage({ type: "close", code: code || 1000, reason: reason || "" });
      // The shell answers with a `close`; if the tab has gone away it never will, so end here too.
      setTimeout(() => this.finish(code || 1000, reason || "", true), 1000);
    }
  }

  for (const [name, value] of [["CONNECTING", CONNECTING], ["OPEN", OPEN], ["CLOSING", CLOSING], ["CLOSED", CLOSED]]) {
    MeshWebSocket[name] = value;
    MeshWebSocket.prototype[name] = value;
  }

  window.WebSocket = MeshWebSocket;

  // An `EventSource` would be answered correctly by the worker already — it is a `fetch` — so it is
  // deliberately left alone. Only the websocket needs replacing.
})();
