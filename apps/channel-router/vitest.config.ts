import { defineWorkersConfig, readD1Migrations } from "@cloudflare/vitest-pool-workers/config";

// Unlike oauth-router's plain-Node suite, this router's logic is inseparable from two Workers
// primitives (Durable Objects, D1) that Node doesn't have -- so the suite runs inside the real
// Workers runtime (miniflare) via this pool, reading its bindings straight from wrangler.toml.
export default defineWorkersConfig(async () => {
  // `readD1Migrations` only runs here, in plain Node at config-build time -- `TEST_MIGRATIONS`
  // below is what hands the parsed migrations into the Workers runtime, where `test/migrate.ts`
  // applies them via `applyD1Migrations(env.ROUTING_KEYS, env.TEST_MIGRATIONS)`.
  const migrations = await readD1Migrations("./migrations");
  return {
    test: {
      include: ["test/**/*.test.ts"],
      poolOptions: {
        workers: {
          wrangler: { configPath: "./wrangler.toml" },
          // Per-test storage isolation (the default) has a known incompatibility with a
          // Durable Object that holds a WebSocket open past the end of the request that
          // created it -- exactly do.ts's subscribe/queue tests. Every test in this suite
          // already uses a fresh randomised node id per connection it creates, so sharing
          // storage across tests costs nothing in isolation and sidesteps the bug.
          isolatedStorage: false,
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
            },
          },
        },
      },
    },
  };
});
