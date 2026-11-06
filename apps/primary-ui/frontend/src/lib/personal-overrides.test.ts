import { test } from "node:test";
import assert from "node:assert/strict";
import { applyOverrides, entityOverrideKey, parseOverrides, updateOverride } from "./personal-overrides";

test("personal overrides preserve shared data, isolate machines, and restore inheritance", () => {
  const shared = { name: "agent", starred: true, metadata: "shared" };
  const local = entityOverrideKey("agent", "agent");
  const remote = entityOverrideKey("agent", "agent", "remote");
  let overrides = updateOverride({}, local, "starred", false);
  overrides = updateOverride(overrides, local, "metadata", "personal");
  overrides = parseOverrides(JSON.stringify(overrides));
  assert.deepEqual(applyOverrides(shared, overrides, local), { ...shared, starred: false, metadata: "personal" });
  assert.deepEqual(applyOverrides(shared, overrides, remote), shared);
  assert.equal(shared.starred, true);
  overrides = updateOverride(overrides, local, "starred", undefined);
  assert.equal(applyOverrides(shared, overrides, local).starred, true);
  assert.equal(applyOverrides(shared, overrides, local).metadata, "personal");
  assert.deepEqual(updateOverride(overrides, local, "metadata", undefined), {});
});

test("invalid browser data falls back safely", () => {
  for (const raw of [null, "broken", "null", "[]", "3"]) assert.deepEqual(parseOverrides(raw), {});
  assert.deepEqual(parseOverrides('{"a":false,"b":[],"c":{"starred":false}}'), { c: { starred: false } });
});
