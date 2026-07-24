// The adi control panel's service worker — what makes the app installable, and what makes
// the shell open instantly instead of re-downloading the wasm bundle on every launch.
//
// It caches the *shell* only, never data. Everything under `/api/` goes straight to the
// network: this is a control panel over live local state, and a stale port table or agent
// list would be worse than an error. Offline you get the frame and an honest failure from
// the API calls inside it.
//
// Bump CACHE to retire every previously cached file in one step.
const CACHE = "adi-shell-v1";

// Fetched on install so a cold, offline launch still has a frame to draw. The hashed
// wasm/js/css names aren't known here — they land in the cache on first load instead.
const SHELL = [
  "/",
  "/manifest.webmanifest",
  "/assets/icon-192.png",
  "/assets/icon-512.png",
  "/assets/icon-maskable-192.png",
  "/assets/icon-maskable-512.png",
  "/assets/apple-touch-icon.png",
  "/assets/favicon.png",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE);
      // Individually, so one 404 can't fail the whole install the way addAll() would.
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

// adi-app answers *any* unknown path with index.html, so a request for an asset that no
// longer exists comes back as a 200 full of HTML. Caching that under a `.wasm` URL would
// wedge the app, so only store what's worth storing.
function cacheable(request, response) {
  if (!response || !response.ok || response.type !== "basic") return false;
  const html = (response.headers.get("content-type") || "").includes("text/html");
  return !html || request.mode === "navigate";
}

async function staleWhileRevalidate(request) {
  const cache = await caches.open(CACHE);
  const hit = await cache.match(request);
  const network = fetch(request)
    .then((response) => {
      if (cacheable(request, response)) cache.put(request, response.clone());
      return response;
    })
    .catch(() => null);
  if (hit) return hit; // instant; the refresh above keeps running
  const fresh = await network;
  if (fresh) return fresh;
  return new Response("offline", { status: 503, statusText: "Offline" });
}

// Always try the network for a page load — the shell must reflect the deployed build — and
// fall back to the cached shell so client-side routes still open offline.
async function shellFirstFromNetwork(request) {
  const cache = await caches.open(CACHE);
  try {
    const response = await fetch(request);
    if (cacheable(request, response)) cache.put("/", response.clone());
    return response;
  } catch {
    return (await cache.match(request)) || (await cache.match("/")) ||
      new Response("offline", { status: 503, statusText: "Offline" });
  }
}

self.addEventListener("fetch", (event) => {
  const { request } = event;
  if (request.method !== "GET") return;

  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;
  // Live state, terminal streams, hook logs — never served from a cache.
  if (url.pathname === "/sw.js" || url.pathname.startsWith("/api/")) return;

  if (request.mode === "navigate") {
    event.respondWith(shellFirstFromNetwork(request));
  } else {
    event.respondWith(staleWhileRevalidate(request));
  }
});
