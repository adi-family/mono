// One dashboard is one origin (../README.md): the front door and the mesh gateway both
// proxy by `Host`, never by port, so this dev server never learns — and never needs to learn —
// its own public address.

import path from "node:path";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The hive runner injects $PORT — the port leased for `<dashboard>/frontend` — and nothing else:
// no host, no public port, because this process has no address of its own (see above).
const PORT = Number(process.env.PORT ?? process.env.PORT_HTTP ?? 5173);

// The component library next door (../packages/ui). This dashboard is its live playground, so it
// imports the library's TypeScript SOURCE directly — no build step in the loop, edits hot-reload.
const UI_SRC = path.resolve(import.meta.dirname, "../packages/ui/src");

export default defineConfig({
  plugins: [react(), tailwindcss(), {
    name: "primary-app-metadata",
    transformIndexHtml(html) {
      if (process.env.VITE_APP_API_BASE !== "/api") return html;
      return {
        html: html.replace(/<title>[^<]*<\/title>/, "<title>ADI</title>"),
        tags: [
          { tag: "link", attrs: { rel: "icon", href: "/assets/mark.svg", type: "image/svg+xml" }, injectTo: "head" as const },
          { tag: "link", attrs: { rel: "manifest", href: "/manifest.webmanifest" }, injectTo: "head" as const },
        ],
      };
    },
  }],
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
      // Dev-only mapping. Points at the library's CSS-less barrel, not its `index.ts`, because
      // THIS app owns the Tailwind pipeline (see index.css's `@source`) — the package's own
      // styles.css belongs to its built/distributed form, and pulling it here would compile the
      // Tailwind layer twice.
      "@design-system/ui": path.resolve(UI_SRC, "components.ts"),
    },
    // The library's source imports `react`; force it to THIS app's single copy rather than the
    // one in packages/ui/node_modules — two React instances break hooks ("invalid hook call").
    dedupe: ["react", "react-dom"],
  },
  server: {
    host: "127.0.0.1",
    port: PORT,
    strictPort: true,
    allowedHosts: [".adi"],
    // Let Vite serve the UI library's source, which sits outside this app's root (one level up).
    fs: { allow: [path.resolve(import.meta.dirname, "..")] },
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
