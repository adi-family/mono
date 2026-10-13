/**
 * The describe harness: import a workflow file and print its `on`, `input`, and `output` exports
 * as JSON, nothing else.
 *
 * Run as `bun run describe.ts <path-to-workflow-file>` by `crate::describe`. Deliberately the
 * *only* thing this does — it must never evaluate `workflow(...)`'s function body, since at
 * describe time there is no run, no input, and no `ctx` to give it. `input`/`output` are zod
 * shapes, converted here (with zod's own `z.toJSONSchema`) into plain JSON Schema — the form the
 * execution engine stores on a run's root task ([`adi_tasks::NewTask::output_schema`]) and
 * `adi-tasks` can validate against with no zod of its own.
 *
 * Seeded by the platform from `crates/adi-workflows/templates/engine/describe.ts`; local edits are
 * overwritten on the next start.
 */

import { z } from "zod";

const file = process.argv[2];
if (!file) {
  console.error("usage: bun run describe.ts <workflow-file>");
  process.exit(1);
}

try {
  const mod = await import(file);
  const raw = mod.on;
  const on = Array.isArray(raw) ? raw : raw ? [raw] : [];
  const inputSchema = mod.input ? z.toJSONSchema(mod.input) : null;
  const outputSchema = mod.output ? z.toJSONSchema(mod.output) : null;
  process.stdout.write(JSON.stringify({ on, inputSchema, outputSchema }));
} catch (err) {
  console.error(String(err instanceof Error ? (err.stack ?? err.message) : err));
  process.exit(1);
}
