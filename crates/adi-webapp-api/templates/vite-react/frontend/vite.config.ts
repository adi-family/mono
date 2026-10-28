// One dashboard is one origin (`guides/dashboards.md`): the front door and the mesh gateway both
// proxy by `Host`, never by port, so this dev server never learns — and never needs to learn —
// its own public address.

import path from "node:path";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The hive runner injects $PORT — the port leased for `<dashboard>/frontend` — and nothing else:
// no host, no public port, because this process has no address of its own (see above).
const PORT = Number(process.env.PORT ?? process.env.PORT_HTTP ?? 5173);

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
    },
  },
  server: {
    host: "127.0.0.1",
    port: PORT,
    strictPort: true,
    // The front door's and the mesh gateway's routes both answer under hostnames that end in
    // `.adi` as a literal string — `<label>.adi` locally, `<label>.<node>.n.adi` over the mesh —
    // so this one entry already covers both; Vite's own "a leading dot allows every subdomain"
    // rule needs nothing more specific here.
    allowedHosts: [".adi"],
    hmr: {
      // Deliberately no `port` / `clientPort`: Vite's injected client falls back to
      // `location.port` (and `location.hostname`) whenever neither is set, so the HMR
      // WebSocket reconnects on whatever host and port the browser actually used to load the
      // page — the front door's — rather than this process's own leased port, which only the
      // loopback side can reach. The front door proxies the `Upgrade` request through to this
      // dev server the same way it proxies any other request on this host: by `Host`, splicing
      // the connection whole once the handshake is past (`adi-hive`'s proxy exempts `Upgrade`
      // requests from the single-request-per-connection rule it otherwise applies on a host two
      // services share). A hardcoded port here would be this process's own, which the browser
      // can never reach directly.
    },
  },
});
