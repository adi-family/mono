// Example test: `app.request(...)` runs a route through Hono's own fetch handler with no socket
// opened — the pattern to follow for a new route's own test. Run with `bun test`.

import { describe, expect, it } from "bun:test";

import app from "../app";

describe("GET /status", () => {
  it("answers ok", async () => {
    const res = await app.request("/status");
    expect(res.status).toBe(200);
    const body = (await res.json()) as { ok: boolean };
    expect(body.ok).toBe(true);
  });
});
