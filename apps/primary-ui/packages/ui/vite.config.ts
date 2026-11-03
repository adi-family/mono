import { resolve } from "node:path";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Main distribution build. ES entries — the whole barrel plus one file per component —
// so a consumer can `import { Button } from "@design-system/ui"` (and tree-shake the rest) or
// `import { Button } from "@design-system/ui/button"` to pull a single file. React and the small
// runtime deps are external: the consuming app already ships them, and bundling React twice
// breaks hooks. The Tailwind layer is emitted once as dist/style.css (import it alongside).
export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    cssCodeSplit: false,
    lib: {
      entry: {
        index: resolve(import.meta.dirname, "src/index.ts"),
        button: resolve(import.meta.dirname, "src/components/button.tsx"),
        card: resolve(import.meta.dirname, "src/components/card.tsx"),
        "app-layout": resolve(import.meta.dirname, "src/components/app-layout.tsx"),
        "scroll-sidebar": resolve(import.meta.dirname, "src/components/scroll-sidebar.tsx"),
        "theme-toggle": resolve(import.meta.dirname, "src/components/theme-toggle.tsx"),
        "question-mark": resolve(import.meta.dirname, "src/components/question-mark.tsx"),
        "help-tooltip": resolve(import.meta.dirname, "src/components/help-tooltip.tsx"),
        select: resolve(import.meta.dirname, "src/components/select.tsx"),
        autocomplete: resolve(import.meta.dirname, "src/components/autocomplete.tsx"),
        "multi-select": resolve(import.meta.dirname, "src/components/multi-select.tsx"),
        chat: resolve(import.meta.dirname, "src/components/chat.tsx"),
        "chat-composer": resolve(import.meta.dirname, "src/components/chat-composer.tsx"),
        "chat-tool-call": resolve(import.meta.dirname, "src/components/chat-tool-call.tsx"),
      },
      formats: ["es"],
    },
    rollupOptions: {
      external: [
        "react",
        "react-dom",
        "react/jsx-runtime",
        "@radix-ui/react-slot",
        "@radix-ui/react-tooltip",
        "@radix-ui/react-popover",
        "class-variance-authority",
        "clsx",
        "lucide-react",
        "tailwind-merge",
      ],
      output: {
        entryFileNames: "[name].js",
        chunkFileNames: "chunks/[name]-[hash].js",
        assetFileNames: (asset) =>
          asset.names?.some((n) => n.endsWith(".css")) ? "style.css" : "[name][extname]",
      },
    },
  },
});
