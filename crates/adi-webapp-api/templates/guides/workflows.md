# Workflows

A workflow is one TypeScript file, run with checkpoints so it resumes exactly where it left off
after a crash or a restart, never from the top. Panel: `/workflows`.

## A workflow file

Global: `workflows/*.ts`. Project-scoped: `projects/<id>/workflows/*.ts` (its id becomes
`<project>/<stem>`, so two projects can each have their own `weekly-report`).

```ts
import { schedule, manual, workflow } from "@adi/workflows";
import { z } from "zod";

export const on = schedule("0 5 * * MON", "America/New_York"); // or event(...), manual(), or [...]
export const input = z.object({ topic: z.string() });   // optional
export const output = z.object({ ok: z.boolean() });    // optional

export default workflow(async (ctx, input) => {
  const a = await ctx.step({ key: "fetch", fn: () => fetchSomething() });
  return { ok: true };
});
```

`on` is one starter, or an array of them:

- `schedule(cron, tz, { catchUp }?)` — `cron` is **standard 5-field unix cron**
  (`min hour dom month dow`, e.g. `"0 5 * * MON"`) — the engine normalizes it to its own 6-field
  dialect internally; write 5 fields, never 6, or it is refused with a clear error naming the
  field count. `tz` is an IANA zone name (`"America/New_York"`), never a bare UTC offset — the
  whole point is carrying DST transitions correctly. `catchUp` is `"fire"` (default — a backlog
  after downtime fires exactly the latest missed slot once) or `"skip"` (the backlog is dropped
  entirely). A brand-new schedule anchors at "now" on its first sweep and fires nothing — enabling
  a workflow never retroactively fires today's already-passed slot.
- `event(pattern, where?)` — `pattern` matches dotted event names (`*` one segment, `**` the
  tail, e.g. `adi.tasks.*`). `where` narrows to payload fields that must match exactly.
- `manual()` — only ever starts by a human or another workflow asking for it
  (`{{cli}} workflows run <id>`, or the panel's "Run now").

## Every `ctx.*` call is a checkpoint

`key` is **required and unique within one run** — reusing a key fails the whole run. A saved
checkpoint replays instantly from its journal on a later attempt; nothing above it ever re-runs.

| call | what it does | what's saved |
| --- | --- | --- |
| `ctx.step({key, fn, retry?})` | runs `fn()` once | its return value |
| `ctx.task({key,title,agent\|person,instructions?,input?,output?,timeout?,project?,cwd?})` | creates a real task once (by its stepId) and waits | the task id, and its validated result once done |
| `ctx.ask({key, person, text})` | a human question, as a task | same as `ctx.task` |
| `ctx.run({key, workflow, input?})` | starts (or finds) a child run keyed to this one | the child run's key, and its validated result once done |
| `ctx.waitFor({key, event, where?, timeout?})` | waits for a matching platform event | the log position it started at, then the matched payload |
| `ctx.sleep({key, until})` | waits until a wall-clock time | the wake time |
| `ctx.dir` / `ctx.runId` / `ctx.slot` | the run's own directory / id / due slot (if any) | — (not a checkpoint) |

`ctx.task`/`ctx.run`/`ctx.ask` resolve to your validated `output` shape, or `{timed_out: true}`
if a `timeout` elapses first (never validated against `output` — it isn't that shape). An
underlying task given up on throws `TaskGaveUp`; uncaught, it fails the whole run.

## The two rules that keep a replay honest

**Never call `Date.now()` / `Math.random()` / `crypto.randomUUID()` outside `ctx.step`.** A
replay has to compute the identical thing up to wherever it resumes; only a `ctx.step`'s own
`fn()` — and only the first time it actually runs — may touch the clock, randomness, or the
outside world.

**`ctx.sleep`'s `until` must be a value computed the same way on every attempt** — derived from
`ctx.slot` (a schedule's own due time) or from the workflow's own `input`. Never
`Date.now() + ms`: recomputed fresh on every attempt, it drifts forward each retry and can
starve a sleep that never fires.

**A `ctx.step` with a real external effect must be idempotent.** It can run more than once if the
process dies after its own effect lands but before the save does — check-before-act (read first,
only act if the thing it would do hasn't happened) or a dedupe key (stamp an id on the side effect
itself, and refuse to repeat one already stamped) are both fine; assuming "ran once" is not.

## The self-retrigger guard

Every `adi.workflows.run.started/completed/failed` event carries `caused_by_run` — the parent, for
a `ctx.run` child; the triggering event's own causer, propagated, for an event-started run; absent
for a schedule or a manual start. Starting a workflow from an event whose own causation chain
already includes a run of that **same** workflow — directly, or through up to **8 hops** of other
workflows in between — is refused and logged, so a workflow reacting to its own side effects (e.g.
listening on `adi.tasks.created`, which its own root task's creation also emits) can never loop
forever by accident.

## Events: 30 days, never past the slowest reader

The shared event log keeps roughly 30 days, and is never dropped past whichever subscriber's own
cursor is furthest behind — a `ctx.waitFor` that hasn't been attempted in a week still sees
everything that happened since it started waiting, as long as that gap stays under the retention
window.

## Rewind

`{{cli}} workflows rewind <run> --to <n>` (or the panel's "Rewind to here" on a run's timeline)
discards the run's journal after step `n` and genuinely redoes the discarded work on the next
attempt — a fresh task, a fresh child run, not a replay of the stale answer. **One limit:** a
discarded `ctx.run` step's own child run is only deleted at its own row and root task — rewind
does **not** cascade into that child's own journal or grandchildren.

## As an agent: a dispatched `ctx.task` reads like any other task

A `ctx.task`/`ctx.ask` assigned to you by name is an ordinary task — read and finish it the usual
way:

```
{{cli}} tasks show <id>                        # instructions, input, the exact output shape
{{cli}} tasks complete <id> --result '<json>'  # validated against that shape; refuses with the
                                                # exact path if it doesn't fit
{{cli}} tasks give-up <id> --reason '...'      # when you genuinely can't do it
```

A `ctx.task`/`ctx.ask` assigned to a **person** shows up on the Tasks panel's "Needs your input"
section, with a form rendered from the task's own `output_schema` — submitting it runs through
the same validation `tasks complete --result` does. To be *pushed* a notice rather than checking
the panel, wire an `event` trigger on `adi.tasks.created` (`triggers.md`) that posts to a
connected Telegram/Slack channel when the payload's `assignee` is the person you want notified —
the task event already carries `assignee`/`workflow_run`/`step_key`, and firing once per task
creation is inherently idempotent.

## Opting an agent into dispatch

A ready task assigned to an agent by name is only ever picked up automatically once that agent has
opted in:

```
{{cli}} agents save <name> --dispatch
```

(or the "Dispatch" toggle on that agent's own editor page). A task assigned to a *person* is never
dispatched, whatever this says.

## A worked shape: proof-post (adi-launch)

The one real multi-step, human-reviewed workflow this design is built around: research
(`ctx.step`) → draft, up to 3 `ctx.task` revise rounds → `ctx.ask` Igor to approve/revise/drop (a
48h timeout) → a child `ctx.run` `publish-post`, whose own `ctx.step` check-before-acts so a retry
never double-posts → a `ctx.step` that verifies the post is actually live. Nothing posts anywhere
without the human approval step genuinely resolving "approve" first — on dev, the publish step is
a dry-run stub.

## Do it

- List / show / run by hand / list runs / rewind / enable / disable:
  `{{cli}} workflows list/show/run/runs/rewind/enable/disable`. Panel: `/workflows`.
- A workflow file this store has never been told about is **disabled by default** — its
  `schedule`/`event` triggers won't fire until `{{cli}} workflows enable <id>` (or the panel's
  toggle). `run`/"Run now" always works regardless, which is how a draft is tried before it is
  trusted with its own triggers.
- `{{cli}} workflows runs` derives each run's status fresh every call — nothing is cached:
  `running` / `waiting on task` / `waiting on event` / `waiting on sleep` / `waiting on child` /
  `done` / `failed` / `stuck` (a task it's waiting on has itself hit the dispatch tick's own
  retry/give-up limit).
- `{{cli}} workflows llm help` is the compact version of this guide, written for a model.
