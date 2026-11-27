import type { Env as RouterEnv } from "../src/types";

// Both cloudflare:workers and the test helpers use the shared binding namespace.
declare global {
  namespace Cloudflare {
    interface Env extends RouterEnv {
      TEST_MIGRATIONS: D1Migration[];
    }
  }
}
