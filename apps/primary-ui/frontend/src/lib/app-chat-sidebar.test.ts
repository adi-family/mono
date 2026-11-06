/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";

import type { Conversation } from "./app-chat";
import { conversationAge, conversationSections, matchesConversationFilter, type SidebarSourceState } from "./app-chat-sidebar";

test("conversation age uses milliseconds and handles missing, future, and older activity", () => {
  const now = Date.UTC(2026, 10, 5);
  for (const value of [0, -1, NaN, Infinity]) assert.equal(conversationAge(value, now), null);
  for (const [elapsed, expected] of [
    [-60, "Just now"], [0, "Just now"], [59, "Just now"], [60, "1m ago"],
    [3599, "59m ago"], [3600, "1h ago"], [86400, "1d ago"],
    [604800, "1w ago"], [2592000, "1mo ago"], [31536000, "1y ago"],
  ] as const) assert.equal(conversationAge(now - elapsed * 1000, now), expected);
});

function conversation(runId: string, values: Partial<Conversation> = {}): Conversation {
  return { agent: "designer", runId, title: runId, message: "", lastActivity: 0, running: false, answerable: true, interactive: false, ...values };
}
function state(source: string | undefined, conversations: Conversation[], values: Partial<SidebarSourceState> = {}): SidebarSourceState {
  return { source, conversations, total: conversations.length, isLoading: false, error: null, ...values };
}

test("started-by-me compares the launcher with that source's asking identity, including unnamed local people", () => {
  const unnamed = conversation("local", { launchedBy: "human", askingUser: null });
  const mine = conversation("mine", { source: "studio", launchedBy: "human:igor", askingUser: "igor" });
  const teammate = conversation("theirs", { source: "studio", launchedBy: "human:maria", askingUser: "igor" });
  const unknown = conversation("unknown");
  const machine = conversation("agent", { launchedBy: "agent:planner" });
  assert.equal(matchesConversationFilter(unnamed, "me", null), true);
  assert.equal(matchesConversationFilter({ ...unnamed, launchedBy: "human:" }, "me", null), true);
  assert.equal(matchesConversationFilter(mine, "me", null), true);
  assert.equal(matchesConversationFilter(teammate, "me", null), false);
  assert.equal(matchesConversationFilter(teammate, "mine", null), true);
  assert.equal(matchesConversationFilter(unknown, "mine", null), false);
  assert.equal(matchesConversationFilter(machine, "mine", null), false);
  assert.equal(matchesConversationFilter({ ...unnamed, askingUser: "igor" }, "me", null), false);
});

test("agent stars determine Only starred; pending questions and the exact active source survive every narrowing", () => {
  const starredChat = conversation("chat", { starred: true });
  assert.equal(matchesConversationFilter(starredChat, "starred", null), false);
  assert.equal(matchesConversationFilter({ ...starredChat, agentStarred: true }, "starred", null), true);
  const pending = conversation("question", { pendingQuestion: true, launchedBy: "automation" });
  for (const filter of ["starred", "mine", "me"] as const) {
    assert.equal(matchesConversationFilter(pending, filter, null), true);
    assert.equal(matchesConversationFilter(starredChat, filter, starredChat), true);
    assert.equal(matchesConversationFilter({ ...starredChat, source: "studio" }, filter, starredChat), false);
  }
});

test("flat rows prioritize unanswered questions, running work, awaits and starred chats before recency", () => {
  const rows = [
    conversation("recent", { lastActivity: 999 }),
    conversation("starred", { starred: true, lastActivity: 80 }),
    conversation("awaiting", { awaiting: true, lastActivity: 60 }),
    conversation("running", { running: true, awaiting: true, lastActivity: 40 }),
    conversation("question", { pendingQuestion: true, lastActivity: 1 }),
    conversation("running-newer", { running: true, lastActivity: 50 }),
  ];
  const sections = conversationSections(rows, [state(undefined, rows)], "flat", "");
  assert.deepEqual(sections[0].conversations.map((row) => row.runId), ["question", "running-newer", "running", "awaiting", "starred", "recent"]);
  assert.equal(sections[0].label, undefined);
});

test("machine groups keep distinct keys for this machine and a paired node literally named local", () => {
  const local = conversation("same");
  const remote = conversation("same", { source: "local" });
  const sources = [state(undefined, [local]), state("local", [remote])];
  const sections = conversationSections([local, remote], sources, "machine", "");
  assert.deepEqual(sections.map((section) => section.label), ["This machine", "local"]);
  assert.equal(new Set(sections.map((section) => section.key)).size, 2);
  assert.deepEqual(sections.map((section) => section.conversations[0].source), [undefined, "local"]);
  assert.equal(conversationSections([local], [sources[0]], "machine", "")[0].label, "This machine");
  assert.equal(conversationSections([remote], [sources[1]], "machine", "")[0].label, "local");
});

test("project groups merge display labels while pagination retains each owning source and project id", () => {
  const local = conversation("local-design", { agent: "local-designer" });
  const remote = conversation("remote-design", { source: "studio", agent: "remote-designer" });
  const unfiled = conversation("unfiled", { agent: "other" });
  const sources = [
    state(undefined, [local, unfiled], { groups: [
      { project: "local-id", label: "Design", conversations: [local], total: 10 },
      { label: "No project", conversations: [unfiled], total: 1 },
    ] }),
    state("studio", [remote], { groups: [{ project: "remote-id", label: "Design", conversations: [remote], total: 8 }] }),
  ];
  const sections = conversationSections([local, remote, unfiled], sources, "project", "");
  assert.deepEqual(sections.map((section) => section.label), ["Design", "No project"]);
  assert.deepEqual(sections[0].conversations.map((row) => row.runId), ["local-design", "remote-design"]);
  assert.deepEqual(sections[0].pages, [
    { source: undefined, project: "local-id", total: 10, held: 1 },
    { source: "studio", project: "remote-id", total: 8, held: 1 },
  ]);
  assert.equal(sections[1].pages[0].project, "~");
});

test("search includes source and server project names without creating empty headings for unloaded older rows", () => {
  const row = conversation("draft", { source: "studio", agent: "designer" });
  const sources = [state("studio", [row], { total: 100, groups: [{ project: "p", label: "Workspace", conversations: [row], total: 100 }] })];
  assert.equal(conversationSections([row], sources, "project", "workspace")[0].conversations.length, 1);
  assert.equal(conversationSections([row], sources, "flat", "studio")[0].conversations.length, 1);
  assert.deepEqual(conversationSections([row], sources, "project", "unmatched"), []);
  assert.deepEqual(conversationSections([row], sources, "machine", "unmatched"), []);
  assert.equal(conversationSections([row], sources, "project", "")[0].pages[0].held, 1);
});

test("pagination counts fetched rows before active exemptions and compatibility filters", () => {
  const fetched = conversation("fetched");
  const active = conversation("injected-active");
  const source = state(undefined, [fetched, active], {
    total: 2, held: 1,
    groups: [{ project: "p", label: "Design", conversations: [fetched, active], total: 2, held: 1 }],
  });
  for (const group of ["flat", "machine", "project"] as const) {
    const page = conversationSections([fetched, active], [source], group, "")[0].pages[0];
    assert.equal(page.total - page.held, 1, "an injected active row must not consume the last fetched slot");
  }

  const filtered = state(undefined, [fetched], {
    total: 5, held: 5,
    groups: [{ project: "p", label: "Design", conversations: [fetched], total: 5, held: 5 }],
  });
  for (const group of ["flat", "machine", "project"] as const) {
    const page = conversationSections([fetched], [filtered], group, "")[0].pages[0];
    assert.equal(page.total - page.held, 0, "client filtering must not promise another page of already fetched rows");
  }
});
