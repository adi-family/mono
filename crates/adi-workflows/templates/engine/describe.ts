/**
 * The describe harness: import a workflow file and print its `on` export as JSON, nothing else.
 *
 * Run as `bun run describe.ts <path-to-workflow-file>` by `crate::describe`. Deliberately the
 * *only* thing this does — it must never evaluate `workflow(...)`'s function body, since at
 * describe time there is no run, no input, and no `ctx` to give it.
 *
 * Seeded by the platform from `crates/adi-workflows/templates/engine/describe.ts`; local edits are
 * overwritten on the next start.
 */

const file = process.argv[2];
if (!file) {
  console.error("usage: bun run describe.ts <workflow-file>");
  process.exit(1);
}

try {
  const mod = await import(file);
  const raw = mod.on;
  const on = Array.isArray(raw) ? raw : raw ? [raw] : [];
  process.stdout.write(JSON.stringify({ on }));
} catch (err) {
  console.error(String(err instanceof Error ? (err.stack ?? err.message) : err));
  process.exit(1);
}
