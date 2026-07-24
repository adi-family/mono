# Triggers

A trigger is a code block the stack launches for you. Three kinds:
- `webhook` — an inbound HTTP call runs the block.
- `background` — a supervised, long-lived process.
- `event` — subscribes to platform event names and runs on a match.

Runtimes: `shell` or `typescript`. Panel: `/triggers`.

## Events
The stack publishes dotted topics like `adi.tasks.created`. An `event` trigger subscribes to
name patterns — `*` matches one segment, `**` the tail, so `adi.tasks.*` catches every task
event — and runs whenever a match fires.

- Emit by hand: `adi events emit <name> [--payload …]` or `POST /api/events/emit`.
- Inspect the queue: `adi events list`.
- Exact payload shape: `adi events types <name> --schema` (or `GET /api/triggers` →
  `event_types[].schema`; `event_types[].example` is a concrete sample).

## Do it
- Create, edit, and enable/disable triggers on the `/triggers` panel. Read `GET /api/triggers`
  for the current set and the event catalog before wiring a new one.
