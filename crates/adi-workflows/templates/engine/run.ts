/**
 * The execution harness: one `bun` process per attempt, piping `ctx` checkpoints to the Rust host
 * over stdin/stdout as line-delimited JSON (see `@adi/workflows`' `createCtx` for the wire
 * protocol). Spawned — not detached, the host waits on it — by `crate::attempt`, once per
 * attempt, never per checkpoint.
 *
 * `argv`: `[modulePath, inputJson]` — `modulePath` is the *pinned*, content-addressed copy of the
 * workflow file (never the live path: see `crate::pin`), `inputJson` the run's input, or empty
 * for none. Env: `ADI_RUN_ID`, `ADI_RUN_DIR`, `ADI_SLOT` (schedule runs only, else empty).
 *
 * Exactly one of three terminal lines ends this process, always followed immediately by
 * `process.exit(0)`: `{"type":"result","value":…}` (the workflow returned, validated against its
 * own `output` shape), `{"type":"gave_up","reason":…}` (a `ctx.task`/`ctx.run`/`ctx.ask` step's
 * `TaskGaveUp` reached the top uncaught), or `{"type":"error","message":…}` (anything else
 * thrown — a bad `input`, a reused step key, a plain exception). A *fourth* possible line,
 * `{"type":"suspend"}`, means a checkpoint has nothing resolved yet: nothing wrong, just nothing
 * further to do this attempt. The explicit `exit(0)` after every one of these matters: an open
 * stdin reader otherwise keeps `bun` alive even with nothing left to read or write.
 *
 * Seeded by the platform from `crates/adi-workflows/templates/engine/run.ts`; local edits are
 * overwritten on the next start.
 */

import { Suspend, TaskGaveUp, createCtx } from "@adi/workflows";

function done(message: Record<string, unknown>): never {
  process.stdout.write(`${JSON.stringify(message)}\n`);
  process.exit(0);
}

function fail(message: string): never {
  done({ type: "error", message });
}

const [modulePath, inputJson] = process.argv.slice(2);
const runId = process.env.ADI_RUN_ID ?? "";
const dir = process.env.ADI_RUN_DIR ?? "";
const slot = process.env.ADI_SLOT ?? "";

if (!modulePath || !runId) {
  fail("usage: bun run run.ts <pinned-module-path> [input-json] (env ADI_RUN_ID, ADI_RUN_DIR)");
}

let mod: Record<string, unknown>;
try {
  mod = await import(modulePath);
} catch (err) {
  fail(`couldn't import the workflow module: ${err instanceof Error ? (err.stack ?? err.message) : String(err)}`);
}

let input: unknown;
try {
  input = inputJson ? JSON.parse(inputJson) : undefined;
} catch (err) {
  fail(`couldn't parse the run's input JSON: ${err instanceof Error ? err.message : String(err)}`);
}

const inputShape = mod.input as { safeParse?: (v: unknown) => { success: boolean; data?: unknown; error?: unknown } } | undefined;
if (inputShape?.safeParse) {
  const parsed = inputShape.safeParse(input);
  if (!parsed.success) {
    fail(`input did not match the workflow's declared shape: ${parsed.error}`);
  }
  input = parsed.data;
}

const body = mod.default as { run?: (ctx: unknown, input: unknown) => Promise<unknown> } | undefined;
if (!body || typeof body.run !== "function") {
  fail("the workflow file has no default workflow(...) export");
}

const ctx = createCtx({ runId, dir, slot });

try {
  let output = await body.run(ctx, input);
  const outputShape = mod.output as
    | { safeParse?: (v: unknown) => { success: boolean; data?: unknown; error?: unknown } }
    | undefined;
  if (outputShape?.safeParse) {
    const parsed = outputShape.safeParse(output);
    if (!parsed.success) {
      fail(`output did not match the workflow's declared shape: ${parsed.error}`);
    }
    output = parsed.data;
  }
  done({ type: "result", value: output ?? null });
} catch (err) {
  if (err instanceof Suspend) {
    done({ type: "suspend" });
  } else if (err instanceof TaskGaveUp) {
    done({ type: "gave_up", reason: err.reason });
  } else {
    fail(err instanceof Error ? (err.stack ?? err.message) : String(err));
  }
}
