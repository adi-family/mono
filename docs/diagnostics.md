# Shared diagnostic log

The dashboard's Event log reads persistent records from `events/log.db` in the active ADI
store (`~/.adi/mono` by default, respecting `ADI_DIR` and the selected flavor). SQLite WAL
supports independent local writers. Records survive browser tabs and process restarts.
Retention is 30 days or 100,000 records, whichever is reached first; it is applied on writes.
This is local storage, not an external telemetry upload.

Each record has a monotonic id, collection timestamp in milliseconds, level (`error`,
`warning`, `info`), source process, area, message, optional JSON context, and optional
idempotency key. Replaying an event key does not create another record. Records are at most
64 KiB. Do not put credentials, request bodies, or environment dumps in messages/context.

## Producers

Rust processes call `adi_events::Events::report(&Diagnostic)` using the same `Config` as
their other stores. A tracing registry can attach `DiagnosticLayer`; it persists WARN and
ERROR independently of the console filter. Storage errors go to stderr without recursion.

The app, Hive/mesh logging initialization, channels daemon, and LLM gateway install the
layer. They require an updated binary/restart to use it. No daemon is restarted by adding
the library. The CLI records command errors, including invalid MCP launch arguments.

Claude session readers import MCP connection/authentication failures, failed tool results,
and failed final results when folding a turn. Agent, ADI session, engine session, and raw
log path are retained as context. This also imports existing raw logs when next read.
The collection timestamp is when the record entered the store, not the historical failure
time. Successful tool output and model thinking are not imported.

The app records synchronous API error responses without request bodies/query strings. The
browser forwards reported background errors and keeps a local fallback if storage is offline.
Uninstrumented third-party processes and arbitrary stderr files are not automatically ingested.

```sh
adi-mono logs report --source backup-job --area storage \
  --context '{"job":"nightly"}' 'Backup failed: disk full'
adi-mono logs list --level error --search MCP --json
adi-mono logs list --source claude --before 123 --limit 100 --json
```

HTTP producers can `POST /api/diagnostics/report` with the same fields:

```json
{"level":"error","source":"backup-job","area":"storage","message":"Disk full","context":{"job":"nightly"},"event_key":"nightly:2026-11-16:disk-full"}
```

## Retrieval

`GET /api/diagnostics` reads the latest 100 records. `POST /api/diagnostics/query` accepts
`source`, `level`, literal `search`, `before` (exclusive record id), and `limit` (1–500).
It returns `{ "records": [...] }`, newest first. Search includes source, area, message, and
JSON context. Reading never acknowledges events on the workflow bus or deletes history.

The dashboard defaults to this machine. Event log's Machine selector can switch to any
paired node, using the existing authenticated node proxy. Selection is remembered per tab;
record ids, caches and unread state are scoped to their owning machine. Each row names the
machine separately from its source process. Only the selected node is polled every ten
seconds. Locked nodes need unlocking/admin access, and nodes must run a build that provides
the diagnostics API. Unavailable nodes show a retrieval error, not another node's records.
Open Event log to search stored history, filter severity, inspect context, or load older pages. Refresh
reloads the current query. Read/unread state is per browser tab. Errors reading the store
are shown in the panel while cached entries remain available.

This feature ships in both the native app and `adi-mono`; deploy matching builds. In
particular, an old app invokes `adi-mono mcp --agent …`, while current builds use
`adi-mono mcp-serve --agent …`. Mixing those builds breaks the agent's MCP tools.
