import { defineConfig } from "vitest/config";

// The router's logic is portable Web-standard code (URL, crypto.subtle, fetch), all of
// which Node provides — so the suite runs in a plain Node environment, no Workers pool.
export default defineConfig({
  test: {
    environment: "node",
    include: ["test/**/*.test.ts"],
  },
});
