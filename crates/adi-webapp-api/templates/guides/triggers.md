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

- Emit by hand: `{{cli}} events emit <name> [--payload …]` or `POST /api/events/emit`.
- Inspect the queue: `{{cli}} events list`.
- Exact payload shape: `{{cli}} events types <name> --schema` (or `GET /api/triggers` →
  `event_types[].schema`). Read the **schema**, not `event_types[].example`: the example omits
  nullable fields, so `adi.tasks.created` looks like it carries no `project`/`tag`/`assignee`
  when in fact it carries all three — and those are exactly what a trigger routes on.
- The catalog is `adi.tasks.*` and `adi.agents.*`, nothing more. No project or hive lifecycle
  event is published, so a trigger can *act* on a service but cannot *react* to one changing.

⚠️ Over the API, `payload` is a **string**, not an object — the body is
`{"name":"…","payload":"{\"from\":\"me\"}"}`. Passing an object gets you
`400 expected JSON body { "name": "…", "payload"?: "…" }`. The trigger receives that string
verbatim as `$ADI_PAYLOAD`, so it is JSON only by convention. The CLI takes it the same way:
`{{cli}} events emit adi.test.probe --payload '{"from":"me"}'`.

## A trigger that launches agents is capped
A block that calls `adi-agents run` is an *automatic* launch, so it is bound by the run caps (see
`agents.md`): the global `max_concurrent_runs` (3 by default) and, if the agent is filed under a
project, that project's own. At a cap the run is refused rather than added, and the block sees a
non-zero exit with the reason — including which cap is full — on stderr. Retry on the next event,
or raise the limit; `--force` belongs to a human at a keyboard, not to a trigger firing unattended.

A per-project cap is the usual way to keep one noisy pipeline from eating the whole machine: give
the project 2 and the rest of the stack keeps its slots.

## Do it
- Create, edit, and enable/disable triggers on the `/triggers` panel. Read `GET /api/triggers`
  for the current set and the event catalog before wiring a new one.
