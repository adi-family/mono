// Agent-authored, never touched by migration — unlike `index.ts` beside it. Hono's typed RPC
// client needs the *whole* route tree in one static type, so (unlike the dependency-free
// template's `backend/routes/`) a new route is two edits, not one: write `routes/<name>.ts`,
// then `.route()` it in here. See ../README.md.

import { Hono } from "hono";

import { statusRoute } from "./routes/status";
import { echoRoute } from "./routes/echo";

const app = new Hono().route("/status", statusRoute).route("/echo", echoRoute);

// What `frontend/src/lib/api.ts`'s `hc<AppType>("/api")` imports — the single source of truth
// the typed client is checked against. Never import anything *from* `index.ts` here: that file
// pulls in Bun's own globals, and this one is also loaded (for its type alone) by the frontend's
// typecheck, which has no reason to know about Bun.
export type AppType = typeof app;

export default app;
