/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";

import { appendEvent, emptyEventLog, eventCollapseMs, eventLogLimit, machineLog, mergeDiagnostics, parseEventLog, queryDiagnostics, unreadEvents, type AppEventInput } from "./event-log";

const failure: AppEventInput = { level: "error", area: "Chats", source: "hetzner-nosh", message: "The app returned an invalid response." };

test("equal record ids on separate machines never collide or cross filters", () => {
  const row = { id: 42, recorded_at: 1500, level: "error" as const, source: "claude", area: "MCP", message: "Failed" };
  let log = mergeDiagnostics(emptyEventLog, [row]);
  log = mergeDiagnostics(log, [row], "claude");
  log = mergeDiagnostics(log, [row], "hetzner-nosh");
  assert.equal(log.events.length, 3);
  assert.deepEqual(machineLog(log).events.map((e) => e.id), ["stored:42"]);
  assert.deepEqual(machineLog(log, "claude").events.map((e) => e.id), ["stored:claude:42"]);
  const readLocal = { ...log, seenAt: 2000, seenByMachine: { claude: 2000 } };
  assert.equal(unreadEvents(machineLog(readLocal, "claude")).length, 0);
  assert.equal(unreadEvents(machineLog(readLocal, "hetzner-nosh")).length, 1);
  assert.equal(parseEventLog(JSON.stringify(readLocal)).seenByMachine?.claude, 2000);
});

test("remote browser records cannot remove a local browser fallback", () => {
  const local = appendEvent(emptyEventLog, failure, 1000, "local-browser");
  const remote = { id: 1, recorded_at: 1500, level: "error" as const, source: "browser", area: "Chat", message: "Failed", context: { browser_event_id: "local-browser" } };
  assert.equal(machineLog(mergeDiagnostics(local, [remote], "node")).events.length, 1);
});

test("node routing preserves filters and pagination without sending machine as process source", async () => {
  const original = globalThis.fetch;
  try {
    globalThis.fetch = (async (url, options) => {
      assert.equal(String(url), "/api/app/node/hetzner-nosh/api/diagnostics/query");
      assert.deepEqual(JSON.parse(String(options?.body)), { before: 42, search: "MCP", limit: 100 });
      return new Response(JSON.stringify({ records: [] }));
    }) as typeof fetch;
    assert.deepEqual(await queryDiagnostics({ before: 42, search: "MCP" }, "hetzner-nosh"), []);
    await assert.rejects(queryDiagnostics({}, "../bad"), /valid paired machine/);
    globalThis.fetch = (async () => new Response("", { status: 404 })) as typeof fetch;
    await assert.rejects(queryDiagnostics({}, "node"), /node:.*Update ADI/);
    globalThis.fetch = (async () => new Response(JSON.stringify({ error: "The node runs an older ADI. Update it first." }), { status: 502 })) as typeof fetch;
    await assert.rejects(queryDiagnostics({}, "node"), /older ADI.*Update it first/);
  } finally { globalThis.fetch = original; }
});

test("server polls preserve identity and unread state, and replace a submitted browser fallback", () => {
  const local = appendEvent(emptyEventLog, failure, 1000, "browser-1");
  const row = { id: 42, recorded_at: 1500, level: "error" as const, source: "browser", area: "Chats", message: failure.message,
    context: { browser_event_id: "browser-1" } };
  const once = mergeDiagnostics(local, [row]);
  assert.equal(once.events.length, 1);
  assert.equal(once.events[0]!.id, "stored:42");
  const seen = { ...once, seenAt: 2000 };
  assert.deepEqual(mergeDiagnostics(seen, [row]), seen);
  assert.equal(unreadEvents(mergeDiagnostics(seen, [row])).length, 0);
});

test("retrieval sends cursor and filters and rejects malformed server rows", async () => {
  const original = globalThis.fetch;
  try {
    globalThis.fetch = (async (_url, options) => {
      assert.deepEqual(JSON.parse(String(options?.body)), { before: 42, level: "error", search: "MCP", limit: 100 });
      return new Response(JSON.stringify({ records: [{ id: 1, recorded_at: "invalid" }] }));
    }) as typeof fetch;
    await assert.rejects(queryDiagnostics({ before: 42, level: "error", search: "MCP" }), /invalid response/);
  } finally { globalThis.fetch = original; }
});

test("a repeat within the collapse window bumps the existing row to the top instead of adding one", () => {
  let log = appendEvent(emptyEventLog, failure, 1_000, "a");
  log = appendEvent(log, { level: "error", area: "Fleet", message: "The app returned HTTP 502." }, 2_000, "b");
  log = appendEvent(log, failure, 3_000, "c");
  assert.deepEqual(log.events.map((event) => [event.id, event.count, event.lastSeen]), [["a", 2, 3_000], ["b", 1, 2_000]]);
  assert.equal(log.events[0]!.firstSeen, 1_000);
});

test("the same message from another machine, at another level or area, is its own row", () => {
  let log = appendEvent(emptyEventLog, failure, 1_000, "a");
  log = appendEvent(log, { ...failure, source: undefined }, 1_001, "b");
  log = appendEvent(log, { ...failure, level: "info" }, 1_002, "c");
  log = appendEvent(log, { ...failure, area: "Chat" }, 1_003, "d");
  assert.deepEqual(log.events.map((event) => event.id), ["d", "c", "b", "a"]);
  assert.equal("source" in log.events[2]!, false, "an absent source is not stored as undefined");
});

test("a repeat after the collapse window starts a new row, and the log is capped", () => {
  let log = appendEvent(emptyEventLog, failure, 1_000, "a");
  log = appendEvent(log, failure, 1_000 + eventCollapseMs + 1, "b");
  assert.deepEqual(log.events.map((event) => event.id), ["b", "a"]);
  for (let index = 0; index < eventLogLimit + 5; index++) log = appendEvent(log, { level: "info", area: "Test", message: `event ${index}` }, 10_000_000 + index, `e${index}`);
  assert.equal(log.events.length, eventLogLimit);
  assert.equal(log.events[0]!.id, `e${eventLogLimit + 4}`);
});

test("unread counts rows seen after the log was last opened", () => {
  let log = appendEvent(emptyEventLog, failure, 1_000, "a");
  log = { ...log, seenAt: 1_500 };
  assert.deepEqual(unreadEvents(log), []);
  log = appendEvent(log, { level: "info", area: "Chats", source: "hetzner-nosh", message: "Responding again." }, 2_000, "b");
  log = appendEvent(log, failure, 2_500, "c");
  assert.deepEqual(unreadEvents(log).map((event) => event.id), ["a", "b"], "a bumped row counts as unread again");
});

test("stored logs are validated row by row and anything unusable falls back to empty", () => {
  assert.deepEqual(parseEventLog(null), emptyEventLog);
  assert.deepEqual(parseEventLog("not json"), emptyEventLog);
  assert.deepEqual(parseEventLog(JSON.stringify({ events: "nope" })), emptyEventLog);
  const good = appendEvent(emptyEventLog, failure, 1_000, "a").events[0];
  const parsed = parseEventLog(JSON.stringify({ events: [good, { id: "bad" }, { ...good, id: "b", level: "fatal" }], seenAt: "soon" }));
  assert.deepEqual(parsed, { events: [good], seenAt: 0 });
});
