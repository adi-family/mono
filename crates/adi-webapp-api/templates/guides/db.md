# Database

A shared **SQLite** database every agent, tool, trigger, and dashboard can read and write. Use it
whenever something has to outlive one run or be seen by someone else — notes, findings, a queue, a
cache, anything you'd otherwise scatter across ad-hoc JSON files.

Two scopes, the usual convention: **global** (shared by everything) or filed under a **project**
(its own separate database file).

## Where it lives
- `~/.adi/mono/db/global.db` — the global database.
- `~/.adi/mono/db/projects/<project-id>.db` — a project's own database.
- `$ADI_DB` is exported into every agent, tool, and trigger run, already pointing at the right
  scope. Prefer it over hardcoding a path.
- The `.db-wal` / `.db-shm` files beside it are SQLite's business — never edit or delete them.

## Do it (shell)
The `adi-db` tool (enable it on the agent) wraps the same commands:

```sh
adi-db exec "create table if not exists notes (id integer primary key, body text not null, at integer)"
adi-db exec "insert into notes (body, at) values (?1, ?2)" --param "found it" --param 1700000000
adi-db query "select * from notes order by at desc limit 5"
adi-db query "select id, body from notes where id = ?1" --param 3 --json
adi-db tables            # what exists, with columns and row counts
adi-db schema notes      # the create statement, before you query someone else's table
adi-db --project acme query "select * from things"   # a project's database
```

- **Always pass values with `--param`, never paste them into the SQL.** It's the injection boundary,
  and it binds numbers as numbers — SQLite compares text `'5'` and integer `5` as *unequal*, so an
  interpolated value silently matches nothing.
- Long or heavily quoted SQL goes on stdin instead of argv: `adi-db exec < migration.sql`.
- `adi-db backup /tmp/snap.db` before anything destructive. It's one command and it's consistent.

## Do it (Bun / TypeScript)
`@adi/db` is already installed in the store, so any `.ts` the platform runs — a tool's script, a
dashboard backend, a `ts` trigger — imports it with no install step:

```ts
import { query, run, get, tx } from "@adi/db";

run("create table if not exists notes (id integer primary key, body text not null, at integer)");
run("insert into notes (body, at) values (?, ?)", "found it", Date.now());

const recent = query<{ id: number; body: string }>("select * from notes order by at desc limit 5");
const one = get<{ body: string }>("select body from notes where id = ?", 3);

// Read-modify-write goes in a transaction — it takes the write lock up front, so a concurrent
// writer becomes a short wait instead of a failure.
tx(() => {
  const n = get<{ n: number }>("select count(*) as n from notes")!.n;
  run("insert into notes (body, at) values (?, ?)", `count was ${n}`, Date.now());
});
```

Also exported: `open({ project, readonly })` for another scope, `dbPath()`, `tables()`, `close()`.

## Notes
- **It's the same file from both sides.** A table you create with `adi-db` is there in `@adi/db` a
  second later, and vice versa — one engine, one file, WAL mode so concurrent readers and a writer
  don't block each other.
- **You are sharing this database.** Name tables for what they hold and who owns them
  (`inbox_triage`, not `data`); use `create table if not exists`; never `drop` or `alter` a table you
  didn't create. Check `adi-db tables` first.
- **Project scope for project data.** If the work belongs to a project, use `--project <id>` (or
  file the agent/tool under it and `$ADI_DB` points there automatically) so it stays separable.
- SQLite has no separate server: nothing to start, nothing that can be "down". If a write ever
  reports the database is locked, another writer held it for more than 5 seconds — retry once.
