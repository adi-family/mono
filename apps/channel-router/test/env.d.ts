import type { Env } from "../src/types";

// Teaches `cloudflare:test`'s `env` helper this Worker's actual binding shape -- without this,
// `env.ROUTING_KEYS` etc. type as `unknown` in every test file.
declare module "cloudflare:test" {
  interface ProvidedEnv extends Env {
    TEST_MIGRATIONS: D1Migration[];
  }
}
