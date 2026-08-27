// The service worker: what makes a node's control panel a page in this browser.
//
// A panel is a whole single-page app — HTML, wasm, CSS, fonts, `/api/*` calls — and it fetches all
// of that with **root-absolute URLs**, because `docs/fleet.md` §4 requires a dashboard never to
// learn its own address. There is no way to serve such an app from a subpath, and no way to give
// each node a real origin without a wildcard domain and a certificate per fleet. So the app is
// served at the origin root and the *client id* decides which node it belongs to.
//
//   1. The shell opens an iframe at `/n/<petname>/` — or `/n/<service>.<petname>/` for one of the
//      dashboards that node runs, which is the same app served from the same origin root.
//   2. This worker sees a navigation there, remembers `resultingClientId -> target`, and answers
//      it from the node — with `js/panel-shim.js` injected into the head.
//   3. That shim immediately rewrites the iframe's URL to `/`, so the panel's own router, its
//      pushState and the browser's back button behave exactly as they do at `app.adi`.
//   4. Every later request from that client id — `/api/health`, a wasm chunk, a font — is answered
//      from the same node, whatever its path.
//
// The map is persisted, because a service worker is stopped whenever the browser feels like it and
// a panel that lost its node on an idle timer would be a panel that breaks after lunch.
//
// This worker **caches the shell and never a node's bytes**. A control panel is live local state;
// a cached port table would be worse than an error. And a node's response cached on this origin
// would be one node's data readable by the next node opened here.

const CACHE = "adi-mesh-client-v1";

// The shell's own files, fetched on install so a cold offline launch still draws a frame. The
// hashed wasm/js names are not known here — they land in the cache on first load instead.
const SHELL = ["/", "/manifest.webmanifest", "/__adi/panel-shim.js"];

// `/n/<target>/…` is the one reserved path on this origin. The target is a node's mesh hostname
// with the fleet zone taken off — `<service>.<petname>`, the same shape `docs/fleet.md` §1
// addresses as `nosh.laptop-b.n.adi` — and a bare `<petname>` means that node's own control panel.
// Every label is a DNS label (§2); which of them is the node is decided in `src/bridge.rs`, since
// a service name may itself be several (`app.nosh`).
const PANEL = /^\/n\/([a-z0-9][a-z0-9-]{0,62}(?:\.[a-z0-9][a-z0-9-]{0,62})*)(\/.*)?$/;

// The other reserved path: this client's own files that a *panel* has to be able to load. A node
// that happened to serve something under this prefix would find it shadowed here, which is the
// trade — one path name, in exchange for the shim being reachable from inside a panel at all.
const RESERVED = "/__adi/";

// clientId -> `<service>.<petname>` (or a bare petname), for pages already open. Mirrored into
// IndexedDB below.
const nodeFor = new Map();

// The port the host page answers mesh requests on. One at a time: whichever tab last announced
// itself owns the endpoint, and a second tab announcing takes over.
let host = null;
let nextId = 1;
const pending = new Map();

// --- the client -> node map, persisted ---------------------------------------------------

const DB = "adi-mesh-client";
const STORE = "kv";
const MAP_KEY = "sw:panels";

function idb() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB, 1);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains(STORE)) {
        request.result.createObjectStore(STORE);
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function loadMap() {
  try {
    const database = await idb();
    const value = await new Promise((resolve, reject) => {
      const tx = database.transaction(STORE, "readonly");
      const request = tx.objectStore(STORE).get(MAP_KEY);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    for (const [id, node] of JSON.parse(value || "[]")) nodeFor.set(id, node);
  } catch {
    // A map we could not read is a map we rebuild as panels re-announce themselves. Not fatal.
  }
}

// Kept warm at the top level so the very first fetch after a worker restart already has it.
const ready = loadMap();

async function rememberMap() {
  try {
    const database = await idb();
    const tx = database.transaction(STORE, "readwrite");
    tx.objectStore(STORE).put(JSON.stringify([...nodeFor]), MAP_KEY);
  } catch {
    // Persisting is an optimisation over the shim's own re-announcement; losing it costs a reload.
  }
}

function remember(clientId, node) {
  if (!clientId || nodeFor.get(clientId) === node) return;
  nodeFor.set(clientId, node);
  // Bounded, so a long-lived install does not accumulate every panel ever opened. The oldest
  // entries are the least likely to still name a live client.
  while (nodeFor.size > 64) nodeFor.delete(nodeFor.keys().next().value);
  rememberMap();
}

// --- lifecycle ---------------------------------------------------------------------------

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE);
      // Individually, so one 404 cannot fail the whole install the way addAll() would.
      await Promise.all(SHELL.map((url) => cache.add(url).catch(() => {})));
      await self.skipWaiting();
    })(),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const names = await caches.keys();
      await Promise.all(names.filter((n) => n !== CACHE).map((n) => caches.delete(n)));
      await self.clients.claim();
    })(),
  );
});

// --- the host page's port ----------------------------------------------------------------

self.addEventListener("message", (event) => {
  const message = event.data || {};
  if (message.type === "host" && event.ports[0]) {
    host = event.ports[0];
    host.onmessage = (reply) => deliver(reply.data);
    host.postMessage({ type: "ready" });
  } else if (message.type === "claim") {
    // The shim, from inside a panel, naming its own node. `event.source.id` is the authority on
    // which client that is — the page cannot know its iframe's client id, and this covers the
    // browsers that do not give us `resultingClientId` on the navigation.
    remember(event.source && event.source.id, message.node);
  }
});

/** One message from the host page, routed to the request that is waiting for it. */
function deliver(message) {
  const waiting = pending.get(message.id);
  if (!waiting) return;
  if (message.type === "head") {
    waiting.head(message);
  } else if (message.type === "chunk") {
    waiting.chunk(message.bytes);
  } else if (message.type === "end") {
    pending.delete(message.id);
    waiting.end();
  } else if (message.type === "error") {
    pending.delete(message.id);
    waiting.fail(message.message);
  }
}

// --- serving a node ------------------------------------------------------------------------

/**
 * Ask the host page for `path` on `node` and answer with what it streams back.
 *
 * The body is a ReadableStream and not a buffer, which is the whole reason this indirection
 * exists: the panel's `/api/*` includes responses that do not end, and one that had to be complete
 * before it could be returned would be one the reader waits on forever.
 */
function fromNode(request, node, path, inject) {
  if (!host) {
    return offline(
      "This tab is not holding the mesh open. Open the adi mesh client and try again.",
    );
  }
  const id = nextId++;
  let controller;
  let injected = !inject;
  const body = new ReadableStream({
    start(c) {
      controller = c;
    },
    cancel() {
      // The reader navigated away or the panel closed the connection. Telling the page is what
      // drops the QUIC stream, so the node stops writing into a feed nobody reads.
      pending.delete(id);
      if (host) host.postMessage({ type: "cancel", id });
    },
  });

  const answered = new Promise((resolve) => {
    pending.set(id, {
      head(message) {
        resolve(
          new Response(body, {
            status: message.status,
            statusText: message.statusText || "",
            headers: responseHeaders(message.headers, inject),
          }),
        );
      },
      chunk(bytes) {
        if (!injected) {
          bytes = injectShim(bytes, node);
          injected = true;
        }
        try {
          controller.enqueue(bytes);
        } catch {
          // Enqueueing into a cancelled stream throws; the cancel handler has already tidied up.
        }
      },
      end() {
        try {
          controller.close();
        } catch {
          /* already closed */
        }
      },
      fail(message) {
        try {
          controller.error(new Error(message));
        } catch {
          /* already closed */
        }
        resolve(offline(message));
      },
    });
  });

  // The body has to be read here, not in the page: a `Request` can only be consumed once, and it
  // is not structured-cloneable, so what crosses to the page is its bytes.
  const send = async () => {
    let body = null;
    if (request.method !== "GET" && request.method !== "HEAD") {
      try {
        const buffer = await request.arrayBuffer();
        if (buffer.byteLength) body = new Uint8Array(buffer);
      } catch {
        // A body we could not read is a request we send without one, which the node will answer
        // with its own error — better than failing here with a message about ReadableStreams.
      }
    }
    host.postMessage(
      {
        type: "fetch",
        id,
        node,
        path,
        method: request.method,
        headers: requestHeaders(request),
        body,
      },
      body ? [body.buffer] : [],
    );
  };
  send();
  return answered;
}

/**
 * The headers to send to the node.
 *
 * A deliberate allow-list rather than a copy, because these bytes are spliced into a real HTTP
 * connection on somebody else's machine (`docs/fleet.md` §3 keeps the head verbatim). `Origin`,
 * `Referer` and `Sec-Fetch-*` are dropped: they name *this* origin, which the node has never heard
 * of, and the panel refuses an `/api/*` request whose `Origin` is another site
 * (`adi-app/src/origin.rs`) — absent is what passes. `Cookie` is dropped because a node's cookie
 * has no business being stored against this origin in the first place.
 */
function requestHeaders(request) {
  const keep = [
    "accept",
    "accept-language",
    "content-type",
    "content-length",
    "if-none-match",
    "if-modified-since",
    "range",
  ];
  const out = {};
  for (const name of keep) {
    const value = request.headers.get(name);
    if (value) out[name] = value;
  }
  return out;
}

/**
 * The headers to answer the panel with.
 *
 * `content-length` and `content-encoding` are dropped when the shim is being injected, because the
 * injection changes the length — and a `Content-Length` that disagrees with the body is a response
 * the browser truncates. Hop-by-hop headers go too: they described the node's own connection, not
 * this one.
 */
function responseHeaders(headers, inject) {
  const out = new Headers();
  const drop = new Set([
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
  ]);
  for (const [name, value] of Object.entries(headers || {})) {
    if (drop.has(name)) continue;
    if (inject && (name === "content-length" || name === "content-encoding")) continue;
    out.append(name, value);
  }
  return out;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/**
 * Put the panel shim into the first chunk of a panel's HTML.
 *
 * First thing inside `<head>`, so it runs before any of the panel's own scripts: it rewrites the
 * iframe's URL out from under them, and a script that had already read `location.pathname` would
 * have read the wrong one.
 */
function injectShim(bytes, node) {
  const text = decoder.decode(bytes, { stream: false });
  const tag = `<script src="/__adi/panel-shim.js" data-node="${node}"></script>`;
  const at = text.search(/<head[^>]*>/i);
  if (at < 0) return encoder.encode(tag + text);
  const end = text.indexOf(">", at) + 1;
  return encoder.encode(text.slice(0, end) + tag + text.slice(end));
}

function offline(message) {
  return new Response(message, {
    status: 503,
    statusText: "Node unreachable",
    headers: { "content-type": "text/plain; charset=utf-8" },
  });
}

// --- the shell's own files -------------------------------------------------------------------

function cacheable(response) {
  return response && response.ok && response.type === "basic";
}

async function shellFirstFromNetwork(request) {
  const cache = await caches.open(CACHE);
  try {
    const response = await fetch(request);
    if (cacheable(response)) cache.put("/", response.clone());
    return response;
  } catch {
    return (
      (await cache.match(request)) ||
      (await cache.match("/")) ||
      new Response("offline", { status: 503, statusText: "Offline" })
    );
  }
}

async function staleWhileRevalidate(request) {
  const cache = await caches.open(CACHE);
  const hit = await cache.match(request);
  const network = fetch(request)
    .then((response) => {
      if (cacheable(response)) cache.put(request, response.clone());
      return response;
    })
    .catch(() => null);
  if (hit) return hit; // instant; the refresh above keeps running
  return (await network) || new Response("offline", { status: 503, statusText: "Offline" });
}

// --- the one routing decision ------------------------------------------------------------------

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (url.origin !== self.location.origin) return;

  const panel = PANEL.exec(url.pathname);
  if (panel) {
    const [, node, rest] = panel;
    // A navigation *into* a node: this is where the mapping is born, and the only response that
    // gets the shim injected into it.
    remember(event.resultingClientId || event.clientId, node);
    event.respondWith(
      ready.then(() => fromNode(event.request, node, (rest || "/") + url.search, true)),
    );
    return;
  }

  event.respondWith(
    (async () => {
      await ready;
      const node = nodeFor.get(event.clientId);
      // `/__adi/` is this origin's own, even inside a panel — it holds the shim, and the shim is
      // requested *by* the panel, whose client id is mapped to a node. Without this exception the
      // worker would faithfully forward the request for the shim to the node, get a 404, and the
      // panel would run with no shim at all: its `new WebSocket()` would go to the real network
      // and its URL would keep the `/n/<petname>/` prefix its own router cannot read. That failure
      // is silent everywhere except in the panel, which is why the reservation is stated here.
      if (node && !url.pathname.startsWith(RESERVED)) {
        // A panel asking for something on its own origin — which, for a panel, is its node.
        return fromNode(event.request, node, url.pathname + url.search, false);
      }
      if (event.request.method !== "GET") return fetch(event.request);
      if (url.pathname === "/sw.js") return fetch(event.request);
      return event.request.mode === "navigate"
        ? shellFirstFromNetwork(event.request)
        : staleWhileRevalidate(event.request);
    })(),
  );
});
