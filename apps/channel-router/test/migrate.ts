import { applyD1Migrations, env } from "cloudflare:test";

/** Apply the D1 schema. Called from each test file's own `beforeAll` rather than a shared
 * `setupFiles` -- vitest-pool-workers takes its per-test storage snapshot at a point a
 * `setupFiles` module's mutations don't reliably land inside, but a `beforeAll` in the test
 * file itself does. */
export async function migrate(): Promise<void> {
  await applyD1Migrations(env.ROUTING_KEYS, env.TEST_MIGRATIONS);
}
