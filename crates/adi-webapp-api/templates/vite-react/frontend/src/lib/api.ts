import { hc } from "hono/client";

import type { AppType } from "../../../backend/src/app";

// One origin, relative paths only (see ../../README.md and `guides/dashboards.md`): the backend
// is a sibling hive service claiming `/api` on this very host, never an address of its own.
// Never pass `hc` anything but this relative base — no host, no port, ever.
export const api = hc<AppType>("/api");
