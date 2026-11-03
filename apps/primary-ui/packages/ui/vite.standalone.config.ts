import { resolve } from "node:path";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Standalone build: ONE self-contained ES file (dist/standalone/design-system-ui.js) that a host
// app can drop in and load on every page. Everything is bundled except React/ReactDOM (the host
// provides them). The Tailwind CSS is inlined into the JS as a string and injected as a <style>
// tag at import time (see src/standalone.ts), so there is no separate stylesheet to ship.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: {
    outDir: "dist/standalone",
    emptyOutDir: true,
    cssCodeSplit: false,
    lib: {
      entry: resolve(import.meta.dirname, "src/standalone.ts"),
      formats: ["es"],
      fileName: () => "design-system-ui.js",
    },
    rollupOptions: {
      external: ["react", "react-dom", "react/jsx-runtime"],
    },
  },
});
