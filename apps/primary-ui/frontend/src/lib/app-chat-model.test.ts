/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";
import type { Agent, AgentPeek, AgentTurn, Conversation, ConversationIndex } from "./app-chat";
import {
  buildSourceRows, chooseAgentName, clearOverlayField, confirmsReply, indexRows, latestTurnSeq, newChatHref, patchIndex,
  preferredAgentName, previewText, rememberRows, resolveActiveSource, selectionKeyOf, type SourceRowsInput,
} from "./app-chat-model";
import { conversationKey } from "./app-chat-sources";

const agent = (name: string, extra: Partial<Agent> = {}): Agent => ({ name, running: false, ...extra });
const row = (runId: string, extra: Partial<Conversation> = {}): Conversation => ({
  agent: "designer", runId, title: runId, message: "", running: false, lastActivity: 1, answerable: true, interactive: true,
  launchedBy: "human", askingUser: null, ...extra,
});
const index = (conversations: Conversation[], extra: Partial<ConversationIndex> = {}): ConversationIndex => ({
  conversations, total: conversations.length, askingUser: null, groups: [], ...extra,
});
const turn = (extra: Partial<AgentTurn>) => ({ role: "user", text: "", seq: 0, at: 0, pending: false, queued: false, images: [], ...extra }) as AgentTurn;
const peek = (turns: AgentTurn[], extra: Partial<AgentPeek> = {}) => ({ turns, awaits: [], running: false, answerable: true, interactive: true, ...extra }) as AgentPeek;
const rows = (extra: Partial<SourceRowsInput>) => buildSourceRows({
  agents: [], overlays: {}, filter: "all", group: "machine", active: null, remembered: new Map(), ...extra,
});

test("agent preference skips agents that cannot launch, then favors starred and the fallback agent", () => {
  assert.equal(preferredAgentName([agent("zed"), agent("adi-agent"), agent("alpha")]), "adi-agent");
  assert.equal(preferredAgentName([agent("adi-agent"), agent("zed", { starred: true })]), "zed");
  assert.equal(preferredAgentName([agent("off", { runnable: false }), agent("mute", { caps: { answerable: false } })]), undefined);
});

test("preference ordering leaves the caller's agent list untouched", () => {
  const agents = [agent("zed"), agent("alpha")];
  preferredAgentName(agents);
  assert.deepEqual(agents.map((item) => item.name), ["zed", "alpha"]);
});

test("a desired agent is honored only when it can launch", () => {
  const agents = [agent("alpha"), agent("off", { runnable: false })];
  assert.equal(chooseAgentName(agents, "off"), "alpha");
  assert.equal(chooseAgentName(agents, "alpha"), "alpha");
  assert.equal(chooseAgentName([], "alpha"), "");
});

test("the active source follows the route, then the last launch, then the first selection", () => {
  const sources = { local: true, nodes: ["studio"] };
  const resolve = (query: string, last?: { agent: string; source?: string }) => resolveActiveSource(new URLSearchParams(query), sources, last);
  assert.equal(resolve("node=studio"), "studio");
  assert.equal(resolve("node="), undefined);
  assert.equal(resolve("agent=a", { agent: "a", source: "studio" }), undefined);
  assert.equal(resolve("", { agent: "a", source: "studio" }), "studio");
  assert.equal(resolve("", { agent: "a", source: "gone" }), undefined);
});

test("new chat links carry the node only for a paired machine", () => {
  assert.equal(newChatHref("a b"), "/interface?agent=a+b");
  assert.equal(newChatHref("a", "studio"), "/interface?agent=a&node=studio");
});

test("previews collapse whitespace and stop at a title's length", () => {
  assert.equal(previewText("  one\n\ttwo  "), "one two");
  assert.equal(previewText("x".repeat(200))?.length, 120);
  assert.equal(previewText(undefined), undefined);
});

test("a new chat's selection key is distinct per agent and machine", () => {
  assert.notEqual(selectionKeyOf(null, "a"), selectionKeyOf(null, "a", "studio"));
  assert.notEqual(selectionKeyOf(null, "a"), selectionKeyOf(null, "b"));
  const active = { agent: "a", runId: "r" };
  assert.equal(selectionKeyOf(active, "ignored"), conversationKey(active));
});

test("a failed send counts as delivered only for a matching newer user turn", () => {
  const images = [{ id: "file" }] as AgentTurn["images"];
  const stored = peek([turn({ seq: 4, text: " hello ", images })]);
  assert.equal(confirmsReply(stored, 3, "hello", ["file"]), true);
  assert.equal(confirmsReply(stored, 4, "hello", ["file"]), false);
  assert.equal(confirmsReply(stored, 3, "other", ["file"]), false);
  assert.equal(confirmsReply(stored, 3, "hello", []), false);
  assert.equal(confirmsReply(peek([turn({ role: "assistant", seq: 4, text: "hello", images })]), 3, "hello", ["file"]), false);
  assert.equal(latestTurnSeq(peek([])), -1);
  assert.equal(latestTurnSeq(undefined), null);
});

test("remembered rows keep identity when unchanged and evict the oldest past the limit", () => {
  const first = row("one");
  const empty = new Map<string, Conversation>();
  const once = rememberRows(empty, [first]);
  assert.notEqual(once, empty);
  assert.equal(rememberRows(once, [first]), once);
  const capped = rememberRows(rememberRows(once, [row("two")], 2), [row("three")], 2);
  assert.deepEqual([...capped.values()].map((item) => item.runId), ["two", "three"]);
});

test("index rows include both flat and grouped conversations", () => {
  const grouped = row("grouped");
  const data = index([row("flat")], { groups: [{ label: "Design", conversations: [grouped], total: 1 }] });
  assert.deepEqual(indexRows(data).map((item) => item.runId), ["flat", "grouped"]);
  assert.deepEqual(indexRows(undefined), []);
});

test("rows take the owning agent's details and local overlays, and hidden rows drop out", () => {
  const kept = row("kept");
  const gone = row("gone");
  const result = rows({
    index: index([kept, gone]),
    agents: [agent("designer", { displayName: "Designer", starred: true, project: "p" })],
    overlays: { [conversationKey(kept)]: { title: "Renamed" }, [conversationKey(gone)]: { hidden: true } },
  });
  assert.deepEqual(result.conversations.map((item) => item.title), ["Renamed"]);
  assert.equal(result.conversations[0].agentDisplayName, "Designer");
  assert.equal(result.conversations[0].project, "p");
  assert.equal(result.loaded, 2);
});

test("the open conversation stays listed when its filter would drop it", () => {
  const active = { agent: "designer", runId: "open" };
  const prior = row("open", { title: "Known title", lastActivity: 9 });
  const remembered = rememberRows(new Map(), [prior]);
  assert.equal(rows({ index: index([]), active, remembered }).conversations[0].title, "Known title");

  const snapshot = peek([turn({ text: " first\nmessage " })], { running: true });
  const fromSnapshot = rows({ index: index([]), active, snapshot }).conversations[0];
  assert.equal(fromSnapshot.title, "first message");
  assert.equal(fromSnapshot.running, true);

  assert.deepEqual(rows({ index: index([]), active }).conversations, []);
  assert.deepEqual(rows({ index: index([]), active, remembered, source: "other" }).conversations, []);
  assert.deepEqual(rows({ index: index([]), active, remembered, overlays: { [conversationKey(active)]: { hidden: true } } }).conversations, []);
});

test("project grouping files the open conversation without counting it as loaded", () => {
  const active = { agent: "designer", runId: "open" };
  const remembered = rememberRows(new Map(), [row("open", { project: "p", projectName: "Design" })]);
  const data = index([], { groups: [{ project: "p", label: "Design", conversations: [row("other", { project: "p" })], total: 1 }] });
  const section = rows({ index: data, group: "project", active, remembered }).groups[0];
  assert.deepEqual(section.conversations.map((item) => item.runId), ["other", "open"]);
  assert.equal(section.loaded, 1);

  const created = rows({ index: index([]), group: "project", active, remembered }).groups[0];
  assert.deepEqual({ label: created.label, total: created.total, loaded: created.loaded }, { label: "Design", total: 0, loaded: 0 });
});

test("accepted edits patch or remove the row in flat and grouped lists", () => {
  const target = row("target");
  const data = index([target, row("other")], { groups: [{ label: "Design", conversations: [target], total: 1 }] });
  const key = conversationKey(target);
  const renamed = patchIndex(data, key, { title: "New" });
  assert.equal(renamed.conversations[0].title, "New");
  assert.equal(renamed.groups[0].conversations[0].title, "New");
  const hidden = patchIndex(data, key, { hidden: true });
  assert.deepEqual(hidden.conversations.map((item) => item.runId), ["other"]);
  assert.deepEqual(hidden.groups[0].conversations, []);
});

test("clearing one overlay field preserves other pending edits and removes empty entries", () => {
  const overlays = { chat: { title: "New", starred: true } };
  assert.deepEqual(clearOverlayField(overlays, "chat", "title"), { chat: { starred: true } });
  assert.deepEqual(clearOverlayField({ chat: { title: "New" } }, "chat", "title"), {});
});
