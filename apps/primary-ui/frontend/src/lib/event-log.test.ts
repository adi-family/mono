/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";

import { appendEvent, emptyEventLog, eventCollapseMs, eventLogLimit, parseEventLog, unreadEvents, type AppEventInput } from "./event-log";

const failure: AppEventInput = { level: "error", area: "Chats", source: "hetzner-nosh", message: "The app returned an invalid response." };

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
