// Example route: GET/POST /api/notes — the shared SQLite store (`guides/db.md`), reached through
// `@adi/db`, the platform's own Bun client for it. The same file every tool, trigger and agent
// script reads and writes, so name a table for what it holds and who owns it — `dashboard_notes`,
// not `notes` — and never `drop` or `alter` one you did not create (`adi-db tables` first).
//
// `@adi/db`'s own implementation imports `bun:sqlite` and Node's `process`/`fs`/`os`/`path`, which
// is fine for this file (the backend has `@types/bun`) but would break the frontend's typecheck
// when it pulls this route in for `AppType`'s structural type alone — see
// `frontend/src/types/adi-db.d.ts`, which is what the frontend resolves "@adi/db" to instead.

import { zValidator } from "@hono/zod-validator";
import { query, run } from "@adi/db";
import { Hono } from "hono";
import { z } from "zod";

interface Note {
  id: number;
  body: string;
  at: number;
}

run(
  "create table if not exists dashboard_notes (id integer primary key, body text not null, at integer not null)",
);

const schema = z.object({ body: z.string().min(1) });

export const notesRoute = new Hono()
  .get("/", (c) => {
    const notes = query<Note>("select * from dashboard_notes order by at desc limit 20");
    return c.json({ notes });
  })
  .post("/", zValidator("json", schema), (c) => {
    const { body } = c.req.valid("json");
    const at = Date.now();
    run("insert into dashboard_notes (body, at) values (?, ?)", body, at);
    return c.json({ ok: true, at });
  });
