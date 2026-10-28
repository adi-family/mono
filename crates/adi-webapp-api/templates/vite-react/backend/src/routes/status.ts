// Example route: GET /api/status. The shape every route file follows: a `Hono()` instance with
// its own handlers, registered at "/" — `app.ts` decides what it is mounted under, so this file
// never writes "/status" itself. Exporting the instance (not a bare handler function) is what
// lets `AppType` chain the whole route tree into one typed client.
//
// Written with no import beyond "hono" on purpose: this file's exported value is part of
// `AppType`, which `frontend/src/lib/api.ts` imports for its *type* alone — but TypeScript still
// has to read this file to know that type, so anything it imports (Bun, Node) would leak an
// unmet ambient-global requirement into the frontend's own typecheck. Keep route files to Hono,
// zod and the Web platform's own globals (`Date`, `crypto`, …).

import { Hono } from "hono";

export const statusRoute = new Hono().get("/", (c) =>
  c.json({
    ok: true,
    now: new Date().toISOString(),
  }),
);
