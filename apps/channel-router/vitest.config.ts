import { cloudflareTest, readD1Migrations } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

// Unlike oauth-router's plain-Node suite, this router's logic is inseparable from two Workers
// primitives (Durable Objects, D1) that Node doesn't have -- so the suite runs inside the real
// Workers runtime (miniflare) via this pool, reading its bindings straight from wrangler.toml.
export default defineConfig(async () => {
  // `readD1Migrations` only runs here, in plain Node at config-build time -- `TEST_MIGRATIONS`
  // below is what hands the parsed migrations into the Workers runtime, where `test/migrate.ts`
  // applies them via `applyD1Migrations(env.ROUTING_KEYS, env.TEST_MIGRATIONS)`.
  const migrations = await readD1Migrations("./migrations");
  return {
    plugins: [
      cloudflareTest({
        wrangler: { configPath: "./wrangler.toml" },
        // Durable Object / D1 bindings come from wrangler.toml, since those need the real
        // Workers runtime to back them; these scalars are test-only fixtures layered on top,
        // playing the part `wrangler secret put` / `.dev.vars` play outside tests.
        miniflare: {
          bindings: {
            TEST_MIGRATIONS: migrations,
            ROUTER_SECRET: "test-router-secret",
            ROUTER_ADMIN_SECRET: "test-admin-secret",
            TELEGRAM_BOT_TOKEN: "test-bot-token",
            TELEGRAM_SECRET_TOKEN: "test-webhook-secret",
            TELEGRAM_BOT_USERNAME: "TestAdiBot",
            SLACK_CLIENT_ID: "test-slack-client-id",
            SLACK_CLIENT_SECRET: "test-slack-client-secret",
            SLACK_SIGNING_SECRET: "test-slack-signing-secret",
          },
        },
      }),
    ],
    // Vitest 4 isolates storage per test file. Tests use fresh node IDs for connections
    // within a file, including WebSockets that remain open after a request ends.
    test: { include: ["test/**/*.test.ts"] },
  };
});
