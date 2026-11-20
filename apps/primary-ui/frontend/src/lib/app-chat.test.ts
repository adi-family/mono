/// <reference types="node" />
import { afterEach, test } from "node:test";
import assert from "node:assert/strict";
import {
  AppChatError, answerQuestion, getAgents, getConversation, getConversationIndex, getConversations, getFleetNodes, getSteps,
  replyToConversation, startConversation, stopConversation, uploadImage, attachmentUrl, renameConversation, starConversation, hideConversation, getVoice, transcribeAudio,
  cancelAwait, cancelQueuedMessage, queuedPosition, queuedTurns, rewindConversation, type AgentPeek, type AgentTurn,
  savedAgents, savedConversation, savedConversationIndex, savedFleetNodes, savedVoice,
} from "./app-chat";
import { groupErrorCount } from "./app-chat-activity";

const originalFetch = globalThis.fetch;
const ref = { agent: "design assistant", runId: "run/one" };
const calls: { url: string; init?: RequestInit }[] = [];

function mockResponse(data: unknown, status = 200): void {
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(input), init });
    return Response.json(data, { status });
  }) as typeof fetch;
}

function body(index = 0): unknown {
  return JSON.parse(String(calls[index].init?.body));
}

afterEach(() => {
  globalThis.fetch = originalFetch;
  calls.length = 0;
  delete (globalThis as { window?: unknown }).window;
});

/** Gives the reads a browser store for one test, as `window.localStorage`. */
function browserStore(): Storage {
  const items = new Map<string, string>();
  const storage: Storage = {
    get length() { return items.size; },
    clear: () => items.clear(),
    getItem: (key) => items.get(key) ?? null,
    key: (index) => [...items.keys()][index] ?? null,
    removeItem: (key) => { items.delete(key); },
    setItem: (key, value) => { items.set(key, String(value)); },
  };
  (globalThis as { window?: unknown }).window = { localStorage: storage };
  return storage;
}
const stored = (storage: Storage) => [...Array(storage.length).keys()].map((index) => storage.getItem(storage.key(index)!)).join("\n");

test("agent list uses the relay and forwards abort without returning unrelated fields", async () => {
  const controller = new AbortController();
  mockResponse({ agents: [{ name: "designer", running: true, runnable: true, backend: "process", caps: { answerable: true }, env: { private: "value" } }] });
  assert.deepEqual(await getAgents(controller.signal), [{ name: "designer", running: true, starred: false, runnable: true, backend: "process", caps: { answerable: true } }]);
  assert.equal(calls[0].url, "/api/app/agents");
  assert.equal(calls[0].init?.method, "GET");
  assert.equal(calls[0].init?.signal, controller.signal);
  assert.equal(calls[0].init?.body, undefined);
});

test("fleet discovery is local while each machine's agents retain their own source", async () => {
  mockResponse({ nodes: [{ node: "laptop-b", locked: false }, { node: "studio", locked: true, password: "private" }] });
  assert.deepEqual(await getFleetNodes(), [{ node: "laptop-b", locked: false }, { node: "studio", locked: true }]);
  assert.equal(calls[0].url, "/api/app/fleet/nodes");
  mockResponse({ agents: [{ name: "designer", running: false, starred: true, project: "project-a", created_by: "human" }] });
  assert.deepEqual(await getAgents(undefined, "laptop-b"), [{ name: "designer", source: "laptop-b", running: false, starred: true, project: "project-a", createdBy: "human" }]);
  assert.equal(calls[1].url, "/api/app/node/laptop-b/api/agents");
});

test("index requests server filtering before pagination and preserves source, identity and grouping", async () => {
  mockResponse({ asking_user: "operator", total: 104, groups: [
    { project: "project-a", label: "Product", agents: ["designer"], total: 9 },
    { label: "No project", agents: ["general"], total: 2 },
  ], agents: [
    { name: "designer", runs: [{ run_id: "same-run", title: "Review", launched_by: "human:operator", pending_question: { id: "ask" }, awaits: [{ id: "wake" }], hidden: true, starred: true }] },
    { name: "general", runs: [{ run_id: "general-run" }] },
  ] });
  const index = await getConversationIndex(undefined, "laptop-b", {
    filter: "me", limit: 200, group: "project", groupLimit: 3, groupMore: { "~": 3, "project-a": 6 },
  });
  const request = new URL(calls[0].url, "http://design-system.adi");
  assert.equal(request.pathname, "/api/app/node/laptop-b/api/agents/runs/all");
  assert.deepEqual(Object.fromEntries(request.searchParams), {
    hidden: "false", limit: "200", filter: "me", group: "project", group_limit: "3", group_more: "project-a:6,~:3",
  });
  assert.equal(index.total, 104);
  assert.equal(index.askingUser, "operator");
  assert.equal(index.conversations[0].source, "laptop-b");
  assert.equal(index.conversations[0].launchedBy, "human:operator");
  assert.equal(index.conversations[0].askingUser, "operator");
  assert.equal(index.conversations[0].pendingQuestion, true);
  assert.equal(index.conversations[0].awaiting, true);
  assert.equal(index.conversations[0].hidden, true);
  assert.equal(index.conversations[0].starred, true);
  assert.equal(index.conversations[0].project, "project-a");
  assert.equal(index.conversations[0].projectName, "Product");
  assert.deepEqual(index.groups.map((group) => ({ label: group.label, total: group.total, runs: group.conversations.map((conversation) => conversation.runId) })), [
    { label: "Product", total: 9, runs: ["same-run"] }, { label: "No project", total: 2, runs: ["general-run"] },
  ]);
});

test("All, any-user and starred filters keep their distinct wire values; hidden history is unpaged", async () => {
  mockResponse({ agents: [] });
  for (const filter of ["all", "mine", "starred", "me"] as const) {
    await getConversationIndex(undefined, undefined, { filter });
    const url = new URL(calls.at(-1)!.url, "http://design-system.adi");
    assert.equal(url.searchParams.get("filter"), filter === "all" ? null : filter);
  }
  await getConversations(undefined, "studio", { hidden: true });
  assert.equal(calls.at(-1)!.url, "/api/app/node/studio/api/agents/runs/all?hidden=true");
});

test("every conversation read and action uses its own source, including same-id chats", async () => {
  const remote = { ...ref, source: "laptop-b" };
  mockResponse({ run_id: "created", turns: [], steps: [] });
  assert.deepEqual(await startConversation(ref.agent, "Start remotely", { source: remote.source }), { agent: ref.agent, runId: "created", source: remote.source });
  await getConversation(remote);
  await getSteps(remote, 2, 3, 5);
  await replyToConversation(remote, "Remote reply");
  await answerQuestion(remote, "ask-remote", ["Yes"]);
  await stopConversation(remote);
  await getConversation(ref);
  assert.deepEqual(calls.map((call) => call.url), [
    "/api/app/node/laptop-b/api/agents/run", "/api/app/node/laptop-b/api/agents/run/peek",
    "/api/app/node/laptop-b/api/agents/run/steps", "/api/app/node/laptop-b/api/agents/run/reply",
    "/api/app/node/laptop-b/api/agents/run/answer", "/api/app/node/laptop-b/api/agents/run/stop",
    "/api/app/agents/run/peek",
  ]);
  assert.deepEqual(body(5), { name: ref.agent, run_id: ref.runId });
});

test("invalid source fragments cannot become a transport path", async () => {
  mockResponse({ agents: [] });
  for (const source of ["", "..", "a/b", "a%2fb", "a?x=1", "a#x", "Laptop", "-edge", "edge-", "a.b", "a".repeat(64)]) {
    await assert.rejects(getAgents(undefined, source), /valid paired machine/);
  }
  assert.equal(calls.length, 0);
});

test("conversation index flattens and sorts history using each historical run's capabilities", async () => {
  mockResponse({ agents: [
    { name: "first", answerable: true, interactive: false, caps: { answerable: true, interactive: false }, runs: [
      { run_id: "older", started_at: 100, last_activity: 0, message: "  First\nmessage  ", running: false },
      { run_id: "newer", started_at: 200, last_activity: 600, message: "Opening", title: "Chosen name", running: true, state: "running", caps: { answerable: false, interactive: true } },
    ] },
    { name: "second", answerable: false, interactive: true, runs: [{ run_id: "middle", started_at: 400, message: "", running: false }] },
  ] });
  const conversations = await getConversations();
  assert.equal(calls[0].url, "/api/app/agents/runs/all?hidden=false&limit=100");
  assert.deepEqual(conversations.map((conversation) => conversation.runId), ["newer", "middle", "older"]);
  assert.equal(conversations[0].answerable, false);
  assert.equal(conversations[0].interactive, true);
  assert.equal(conversations[0].title, "Chosen name");
  assert.equal(conversations[1].title, "Untitled chat");
  assert.equal(conversations[2].title, "First message");
  assert.equal(conversations[2].lastActivity, 100);
});

test("peek sends flattened pagination and tolerates future steps and old missing fields", async () => {
  mockResponse({ name: ref.agent, run_id: ref.runId, answerable: true, running: true, total_turns: 50, turns: [
    { role: "user", text: "Hello", seq: 30 },
    { role: "assistant", text: "Working", seq: 31, pending: true, steps: [
      { kind: "future-step", text: "new schema" },
      { kind: "tool", name: "Bash", input: "{}", status: "running" },
      { kind: "calls", from: 4, to: 7, count: 3, tools: ["Bash"], preview: "git status", status: "ok" },
    ], markers: [{ kind: "from", node: "laptop", user: "" }] },
  ] });
  const peek = await getConversation(ref, undefined, { limit: 10, before: 40 });
  assert.deepEqual(body(), { name: ref.agent, run_id: ref.runId, limit: 10, fold: true, before: 40 });
  assert.equal(peek.turns[0].seq, 30);
  assert.deepEqual(peek.turns[0].images, []);
  assert.equal(peek.total_turns, 50);
  assert.deepEqual(peek.turns[1].steps[0], { kind: "unknown" });
  assert.deepEqual(peek.turns[1].steps[1], { kind: "tool", name: "Bash", input: "{}", status: "running", output: "" });
  assert.deepEqual(peek.turns[1].markers, [{ kind: "from", node: "laptop", user: "" }]);
});

test("default peek pages twenty turns and gives old transcripts stable positions", async () => {
  mockResponse({ turns: [{ role: "user", text: "One", seq: 0 }, { role: "assistant", text: "Two", seq: 0 }] });
  const peek = await getConversation(ref);
  assert.deepEqual(body(), { name: ref.agent, run_id: ref.runId, limit: 20, fold: true });
  assert.deepEqual(peek.turns.map((turn) => turn.seq), [0, 1]);
  assert.equal(peek.total_turns, 2);
});

test("folded tool expansion addresses original turn sequence and step range", async () => {
  const controller = new AbortController();
  mockResponse({ steps: [{ kind: "thinking", text: "Plan" }, { kind: "tool", name: "git", input: "{}", output: "Done", status: "error" }] });
  const steps = await getSteps(ref, 42, 3, 5, controller.signal);
  assert.equal(calls[0].url, "/api/app/agents/run/steps");
  assert.equal(calls[0].init?.signal, controller.signal);
  assert.deepEqual(body(), { name: ref.agent, run_id: ref.runId, turn: 42, from: 3, to: 5 });
  assert.equal(steps[1].kind, "tool");
});

test("folded activity counts failures from whole-chat stats even when its last call succeeded", async () => {
  mockResponse({ turns: [], stats: {
    failed: [
      { turn: 2, step: 3, run_from: 0, tool: "Bash", arg: "private preview" },
      { turn: 2, step: 8, run_from: 8, tool: "Edit" },
      { turn: 4, step: 1, tool: "Bash" },
    ],
    by_tool: [{ tool: "Bash", calls: 9, failed: 2 }, { tool: "Edit", calls: 3, failed: 1 }],
    tokens: 100,
  } });
  const { stats } = await getConversation(ref);
  assert.deepEqual(stats, {
    failed: [{ turn: 2, step: 3 }, { turn: 2, step: 8 }, { turn: 4, step: 1 }],
    by_tool: [{ tool: "Bash", calls: 9, failed: 2 }, { tool: "Edit", calls: 3, failed: 1 }],
  });
  assert.equal(groupErrorCount(stats, 2, 0, 8), 1);
  assert.equal(groupErrorCount(stats, 2, 8, 10), 1);
  assert.equal(groupErrorCount(stats, 4, 0, 2), 1);
  assert.equal(groupErrorCount(stats, 3, 0, 20), 0);
});

test("capped failure references only give exact counts through their retained prefix", async () => {
  mockResponse({ turns: [], stats: {
    failed: Array.from({ length: 40 }, (_, step) => ({ turn: 2, step })),
    by_tool: [{ tool: "Bash", calls: 80, failed: 42 }],
  } });
  const { stats } = await getConversation(ref);
  assert.equal(groupErrorCount(stats, 1, 0, 100), 0);
  assert.equal(groupErrorCount(stats, 2, 30, 40), 10);
  assert.equal(groupErrorCount(stats, 2, 30, 41), undefined);
  assert.equal(groupErrorCount(stats, 3, 0, 10), undefined);
  assert.equal(groupErrorCount(stats, 3, 5, 5), 0);
  assert.equal(groupErrorCount(stats, 2, 5, 4), undefined);
});

test("zero failures are known while old or malformed statistics remain unknown", async () => {
  mockResponse({ turns: [], stats: { failed: [], by_tool: [{ tool: "Read", calls: 8, failed: 0 }] } });
  assert.equal(groupErrorCount((await getConversation(ref)).stats, 4, 0, 8), 0);
  const valid = { failed: [{ turn: 2, step: 1 }], by_tool: [{ tool: "Bash", calls: 2, failed: 1 }] };
  for (const stats of [
    undefined,
    { failed: [] },
    { ...valid, failed: [{ turn: 2, step: -1 }] },
    { ...valid, failed: [{ turn: 2, step: 1 }, { turn: 2, step: 1 }] },
    { ...valid, by_tool: [{ tool: "Bash", calls: 1, failed: 2 }] },
    { ...valid, by_tool: [{ tool: "Bash", calls: 2, failed: 0.5 }] },
    { ...valid, by_tool: [{ tool: "Bash", calls: 2, failed: "1" }] },
    { ...valid, by_tool: [{ tool: "Bash", calls: 2, failed: 0 }] },
  ]) {
    mockResponse({ turns: [], stats });
    const peek = await getConversation(ref);
    assert.equal(peek.stats, undefined);
    assert.equal(groupErrorCount(peek.stats, 2, 0, 8), undefined);
  }
});

test("new conversation sends human provenance once and returns the server's run id", async () => {
  mockResponse({ run_id: "created", message: "Started", state: {} });
  assert.deepEqual(await startConversation("designer", "Opening message"), { agent: "designer", runId: "created" });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, "/api/app/agents/run");
  assert.equal(calls[0].init?.method, "POST");
  assert.deepEqual(calls[0].init?.headers, { "Content-Type": "application/json" });
  assert.deepEqual(body(), { name: "designer", message: "Opening message", attachments: [], launched_by: "human" });
});

test("reply preserves text and can queue normally while generation is running", async () => {
  mockResponse({ running: true, turns: [{ role: "user", text: "Keep\nspacing", queued: true }] });
  const peek = await replyToConversation(ref, "Keep\nspacing");
  assert.equal(calls[0].url, "/api/app/agents/run/reply");
  assert.deepEqual(body(), { name: ref.agent, run_id: ref.runId, message: "Keep\nspacing", attachments: [], mode: "regular", limit: 20, fold: true });
  assert.equal(peek.turns[0].queued, true);
});

test("answer pins the pending question id and stop targets only the selected run", async () => {
  mockResponse({ turns: [] });
  await answerQuestion(ref, "ask-one", ["First", "Second"]);
  assert.equal(calls[0].url, "/api/app/agents/run/answer");
  assert.deepEqual(body(), { name: ref.agent, run_id: ref.runId, ask: "ask-one", replies: ["First", "Second"], limit: 20, fold: true });
  await stopConversation(ref);
  assert.equal(calls[1].url, "/api/app/agents/run/stop");
  assert.deepEqual(body(1), { name: ref.agent, run_id: ref.runId });
});

test("HTTP and API failures expose useful errors and never retry a mutation", async () => {
  mockResponse({ ok: false, error: "Run limit reached" }, 429);
  await assert.rejects(startConversation("designer", "Try once"), (error: unknown) => error instanceof AppChatError && error.status === 429 && error.message === "Run limit reached");
  assert.equal(calls.length, 1);
  mockResponse({ ok: false, error: "Question already answered" });
  await assert.rejects(answerQuestion(ref, "stale", ["Yes"]), /Question already answered/);
  assert.equal(calls.length, 2);
});

test("invalid JSON gives a bounded message instead of exposing an upstream document", async () => {
  globalThis.fetch = (async () => new Response("<html>Unexpected upstream page</html>", { status: 502 })) as typeof fetch;
  await assert.rejects(getAgents(), /The app returned HTTP 502\./);
  mockResponse({ agents: "invalid" });
  await assert.rejects(getAgents(), /The app returned an invalid response\./);
});

test("aborts propagate without swallowing or retrying the cancellation", async () => {
  const controller = new AbortController();
  const error = new DOMException("Cancelled", "AbortError");
  controller.abort(error);
  globalThis.fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: "aborted", init });
    throw init?.signal?.reason;
  }) as typeof fetch;
  await assert.rejects(getConversation(ref, controller.signal), (caught: unknown) => caught === error);
  assert.equal(calls.length, 1);
});

test("image uploads use raw bytes, preserve machine routing and send only attachment IDs", async () => {
  const file = new File([new Uint8Array([137, 80, 78, 71])], "shot.png", { type: "image/png" });
  mockResponse({ id: "image-one" });
  const image = await uploadImage(file, "laptop-b");
  assert.equal(calls[0].url, "/api/app/node/laptop-b/api/agents/attachment");
  assert.equal(calls[0].init?.body, file);
  assert.equal(new Headers(calls[0].init?.headers).get("Content-Type"), "image/png");
  assert.equal(attachmentUrl(image.id, "laptop-b"), "/api/app/node/laptop-b/api/agents/attachment/image-one");
  mockResponse({ turns: [] });
  await replyToConversation({ ...ref, source: "laptop-b" }, "", { attachments: [image.id] });
  assert.deepEqual((body(1) as { attachments: string[] }).attachments, ["image-one"]);
  mockResponse({ run_id: "new" });
  await startConversation("designer", "", { source: "laptop-b", attachments: [image.id] });
  assert.deepEqual((body(2) as { attachments: string[] }).attachments, ["image-one"]);
});

test("oversized uploads fail before sending; upload errors remain readable", async () => {
  mockResponse({ error: "Upload refused" }, 413);
  await assert.rejects(uploadImage(new File([new Uint8Array(25 * 1024 * 1024 + 1)], "x.pdf", { type: "application/pdf" })), /25 MB/);
  await assert.rejects(uploadImage(new File([new Uint8Array(5 * 1024 * 1024 + 1)], "x.png", { type: "image/png" })), /5 MB/);
  assert.equal(calls.length, 0);
  await assert.rejects(uploadImage(new File(["x"], "x.png", { type: "image/png" })), /Upload refused/);
});

test("rename targets the owning machine and allows restoring the generated title", async () => {
  mockResponse({ agents: [] });
  await renameConversation({ ...ref, source: "laptop-b" }, "  Design review  ");
  assert.equal(calls[0].url, "/api/app/node/laptop-b/api/agents/run/rename");
  assert.deepEqual(body(), { name: ref.agent, run_id: ref.runId, title: "Design review" });
  await renameConversation(ref, " ");
  assert.equal((body(1) as { title: string }).title, "");
});

test("dictation reuses local engines and sends raw audio rather than a chat reply", async () => {
  mockResponse({ engines: [{ id: "openai", label: "OpenAI", ready: true, in_browser: false, detail: "Configured" }], default_engine: "openai" });
  assert.equal((await getVoice()).default_engine, "openai");
  assert.equal(calls[0].url, "/api/app/voice");
  mockResponse({ text: "Recognized words" });
  const clip = new Blob(["fixture-audio"], { type: "audio/webm;codecs=opus" });
  assert.equal(await transcribeAudio(clip, "openai"), "Recognized words");
  assert.equal(calls[1].url, "/api/app/voice/transcribe?engine=openai");
  assert.equal(calls[1].init?.body, clip);
  mockResponse({ error: "Missing transcription key" }, 400);
  await assert.rejects(transcribeAudio(clip, "openai"), /Missing transcription key/);
});

test("chat stars target the owning source and send explicit desired state", async () => {
  mockResponse({ ok: true });
  await starConversation({ ...ref, source: "laptop-b" }, true);
  assert.equal(calls[0].url, "/api/app/node/laptop-b/api/agents/run/star");
  assert.deepEqual(body(), { name: ref.agent, run_id: ref.runId, starred: true });
  await starConversation(ref, false);
  assert.equal(calls[1].url, "/api/app/agents/run/star");
  assert.deepEqual(body(1), { name: ref.agent, run_id: ref.runId, starred: false });
});

 test("PDF and untyped file uploads preserve bytes and media types", async () => {
  for (const type of ["application/pdf", ""]) {
    mockResponse({ id: "document-one" });
    const file = new File(["%PDF-1.7"], "report.pdf", { type });
    const uploaded = await uploadImage(file, "laptop-b");
    const call = calls.at(-1)!;
    assert.equal(call.url, "/api/app/node/laptop-b/api/agents/attachment");
    assert.equal(call.init?.body, file);
    assert.equal(new Headers(call.init?.headers).get("Content-Type"), type || "application/octet-stream");
    assert.equal(uploaded.media_type, type || "application/octet-stream");
  }
});


test("Hide persists on the owning machine and reports failed saves", async () => {
  mockResponse({ ok: true });
  await hideConversation({ ...ref, source: "laptop-b" });
  assert.equal(calls[0].url, "/api/app/node/laptop-b/api/agents/run/hide");
  assert.deepEqual(body(0), { name: ref.agent, run_id: ref.runId, hidden: true });
  await hideConversation(ref);
  assert.equal(calls[1].url, "/api/app/agents/run/hide");
  mockResponse({ ok: false, error: "Hide refused" });
  await assert.rejects(hideConversation(ref), /Hide refused/);
  assert.equal(calls.length, 3);
});

test("ASAP replies retain attachments and target the owning machine", async () => {
  mockResponse({ running: true, turns: [{ role: "user", text: "Update", queued: true, mode: "asap" }] });
  const peek = await replyToConversation({ ...ref, source: "remote" }, "Update", { attachments: ["file-id"], mode: "asap" });
  assert.equal(calls[0].url, "/api/app/node/remote/api/agents/run/reply");
  assert.deepEqual(body(), { name: ref.agent, run_id: ref.runId, message: "Update", attachments: ["file-id"], mode: "asap", limit: 20, fold: true });
  assert.equal(peek.turns[0].mode, "asap");
});

/** Answers each request with the next response in turn; the last one repeats. */
function mockResponses(...responses: unknown[]): void {
  let next = 0;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(input), init });
    return Response.json(responses[Math.min(next++, responses.length - 1)]);
  }) as typeof fetch;
}

const queuedTurn = (text: string, extra: Record<string, unknown> = {}) => ({ role: "user", text, queued: true, ...extra });
const answered = { role: "user", text: "Earlier question" };

async function queuedOf(...turns: unknown[]): Promise<{ peek: AgentPeek; queue: AgentTurn[] }> {
  mockResponse({ running: true, turns });
  const peek = await getConversation(ref);
  calls.length = 0;
  return { peek, queue: queuedTurns(peek) };
}

test("cancelling a queued message re-reads the queue and drops it by its current position", async () => {
  const { queue } = await queuedOf(answered, queuedTurn("First"), queuedTurn("Second"), queuedTurn("Third"));
  assert.deepEqual(queue.map((turn) => turn.text), ["First", "Second", "Third"]);
  // "First" started its turn after the snapshot on screen was read.
  mockResponses(
    { running: true, turns: [answered, { role: "user", text: "First" }, queuedTurn("Second"), queuedTurn("Third")] },
    { running: true, turns: [answered, { role: "user", text: "First" }, queuedTurn("Third")] },
  );
  const after = await cancelQueuedMessage({ ...ref, source: "studio" }, queue[1], 1);
  assert.deepEqual(calls.map((call) => call.url), [
    "/api/app/node/studio/api/agents/run/peek", "/api/app/node/studio/api/agents/run/unqueue",
  ]);
  assert.deepEqual(body(1), { name: ref.agent, run_id: ref.runId, index: 0, limit: 20, fold: true });
  assert.deepEqual(queuedTurns(after).map((turn) => turn.text), ["Third"]);
});

test("rewinding addresses the message by its turn number on the conversation's own machine", async () => {
  mockResponse({ caps: { rewind: true }, turns: [{ role: "user", text: "First", seq: 0 }, { role: "assistant", text: "Answer", seq: 1 }] });
  const after = await rewindConversation({ ...ref, source: "studio" }, 2);
  assert.deepEqual(calls.map((call) => call.url), ["/api/app/node/studio/api/agents/run/rewind"]);
  assert.deepEqual(body(0), { name: ref.agent, run_id: ref.runId, seq: 2, limit: 20, fold: true });
  assert.deepEqual(after.turns.map((turn) => turn.text), ["First", "Answer"]);
  assert.equal(after.caps.rewind, true);
});

test("a queued message that already started is not cancelled, and no other message is dropped in its place", async () => {
  const { queue } = await queuedOf(queuedTurn("First"), queuedTurn("Second"));
  mockResponse({ running: true, turns: [{ role: "user", text: "First" }, queuedTurn("Second")] });
  const after = await cancelQueuedMessage(ref, queue[0], 0);
  assert.deepEqual(calls.map((call) => call.url), ["/api/app/agents/run/peek"]);
  assert.deepEqual(queuedTurns(after).map((turn) => turn.text), ["Second"]);
});

test("queued positions tell identical text apart by mode, attachments and order", async () => {
  const image = { id: "image-one", name: "one.png", media_type: "image/png", size: 1 };
  const { peek, queue } = await queuedOf(
    queuedTurn("Same"), queuedTurn("Same", { mode: "asap" }), queuedTurn("Same", { images: [image] }), queuedTurn("Same"),
  );
  assert.deepEqual(queue.map((turn, position) => queuedPosition(peek, turn, position)), [0, 1, 2, 3]);
  // A message never moves back, so a later twin is not mistaken for one that has gone.
  const { peek: later } = await queuedOf(queuedTurn("Other"), queuedTurn("Same"));
  assert.equal(queuedPosition(later, queue[0], 0), null);
  assert.equal(queuedPosition(later, queue[3], 3), 1);
});

test("cancelling an await names the conversation and returns what is still pending", async () => {
  mockResponse({ awaits: [{ id: "review", note: "Reviewer result", summary: "on review.done", check: "private-command" }] });
  const remaining = await cancelAwait({ ...ref, source: "studio" }, "build");
  assert.equal(calls[0].url, "/api/app/node/studio/api/agents/await/ignore");
  assert.deepEqual(body(), { name: ref.agent, run_id: ref.runId, id: "build" });
  assert.deepEqual(remaining, [{ id: "review", note: "Reviewer result", summary: "on review.done", check: "private-command" }]);
  mockResponse({ ok: false, error: "await build changed or fired before this reached it" }, 400);
  await assert.rejects(cancelAwait(ref, "build"), /changed or fired/);
  assert.equal(calls.length, 2);
});

test("without a browser store the reads save nothing and nothing is restored", async () => {
  mockResponse({ nodes: [{ node: "studio", locked: false }] });
  assert.deepEqual(await getFleetNodes(), [{ node: "studio", locked: false }]);
  assert.equal(savedFleetNodes(), undefined);
  assert.equal(savedAgents(), undefined);
  assert.equal(savedConversationIndex(undefined, { filter: "me" }), undefined);
  assert.equal(savedConversation(ref), undefined);
  assert.equal(savedVoice(), undefined);
});

test("the paired machines and the voice engines are restored as they were read", async () => {
  const storage = browserStore();
  mockResponse({ nodes: [{ node: "studio", locked: false, key: "private" }, { node: "server", locked: true }] });
  const nodes = await getFleetNodes();
  assert.deepEqual(savedFleetNodes(storage)?.data, nodes);
  mockResponse({ engines: [{ id: "browser", label: "Browser", ready: true, in_browser: true, detail: "Built in" }], default_engine: "browser", api_key: "private" });
  const voice = await getVoice();
  assert.deepEqual(savedVoice(storage)?.data, voice);
  assert.ok(!stored(storage).includes("private"));
});

test("a machine's agents are restored per machine, from the few fields the chat reads", async () => {
  const storage = browserStore();
  mockResponse({ agents: [
    { name: "designer", running: true, starred: true, project: "studio", created_by: "human", runnable: true, backend: "process", caps: { answerable: true, images: true },
      prompt: "A very long private definition", env: { TOKEN: "private" } },
    { name: "plain" },
  ] });
  const local = await getAgents();
  const remote = await getAgents(undefined, "studio");
  assert.deepEqual(savedAgents(undefined, storage)?.data, local);
  assert.deepEqual(savedAgents("studio", storage)?.data, remote);
  assert.equal(savedAgents("studio", storage)?.data[0]?.source, "studio");
  assert.equal(savedAgents("server", storage), undefined);
  assert.ok(typeof savedAgents(undefined, storage)?.savedAt === "number");
  for (const dropped of ["private", "prompt", "env"]) assert.ok(!stored(storage).includes(dropped), `${dropped} is not stored`);
});

const listing = { total: 3, asking_user: "igor", groups: [{ project: "studio", label: "Studio", agents: ["designer"], total: 2, path: "/private/studio" }], agents: [
  { name: "designer", answerable: true, interactive: true, starred: true, caps: { answerable: true, interactive: false, images: true }, definition: "private definition", runs: [
    { run_id: "one", title: "First", message: "Draw it", running: true, last_activity: 300, started_at: 100, launched_by: "human", hidden: false, starred: true, state: "running",
      caps: { answerable: false, interactive: true, rewind: true }, pending_question: { id: "ask-1", note: "private question" }, awaits: [{ id: "w-1", note: "private wait" }], outcome: "private outcome" },
    { run_id: "two", message: "m".repeat(2000), last_activity: 200 },
  ] },
  { name: "reviewer", runs: [{ run_id: "three", message: "Review", started_at: 50 }] },
] };

test("a list of conversations is restored exactly as it was read, marked as saved", async () => {
  const storage = browserStore();
  mockResponse(listing);
  const live = await getConversationIndex(undefined, undefined, { filter: "me", limit: 100 });
  assert.equal(live.saved, undefined, "an answer from the machine is never marked as saved");
  const restored = savedConversationIndex(undefined, { filter: "me", limit: 100 }, storage);
  assert.equal(restored?.data.saved, true);
  const long = live.conversations.find((conversation) => conversation.runId === "two")!;
  assert.equal(long.message.length, 2000);
  // Only the first message's preview is shown anywhere, so the stored copy keeps the start of it.
  assert.deepEqual({ ...restored?.data, saved: undefined }, { ...live, saved: undefined,
    conversations: live.conversations.map((conversation) => conversation === long ? { ...long, message: long.message.slice(0, 600) } : conversation),
    groups: live.groups.map((group) => ({ ...group, conversations: group.conversations.map((conversation) => conversation === long ? { ...long, message: long.message.slice(0, 600) } : conversation) })) });
  assert.deepEqual(restored?.data.conversations.map((conversation) => [conversation.runId, conversation.title, conversation.pendingQuestion, conversation.awaiting, conversation.answerable, conversation.interactive, conversation.projectName]),
    [["one", "First", true, true, false, true, "Studio"], ["two", "m".repeat(120), false, false, true, false, "Studio"], ["three", "Review", false, false, false, false, undefined]]);
  for (const dropped of ["private", "definition", "outcome", "images", "rewind", "path"]) assert.ok(!stored(storage).includes(dropped), `${dropped} is not stored`);
});

test("each machine and view has its own saved list, and only the first page is kept", async () => {
  const storage = browserStore();
  mockResponse(listing);
  await getConversationIndex(undefined, undefined, { filter: "me" });
  await getConversationIndex(undefined, "studio", { filter: "me", group: "project", groupLimit: 3 });
  assert.equal(savedConversationIndex(undefined, { filter: "me" }, storage)?.data.conversations.length, 3);
  assert.equal(savedConversationIndex("studio", { filter: "me", group: "project", groupLimit: 3, groupMore: {} }, storage)?.data.conversations[0]?.source, "studio");
  for (const other of [{ filter: "all" as const }, { filter: "me" as const, group: "project" as const }]) assert.equal(savedConversationIndex(undefined, other, storage), undefined);
  assert.equal(savedConversationIndex("studio", { filter: "me" }, storage), undefined);
  // Longer pages, expanded projects and the hidden list are read but never stored or restored.
  const before = stored(storage);
  for (const page of [{ filter: "me" as const, limit: 200 }, { filter: "me" as const, hidden: true }, { filter: "me" as const, group: "project" as const, groupMore: { studio: 3 } }]) {
    await getConversationIndex(undefined, undefined, page);
    assert.equal(savedConversationIndex(undefined, page, storage), undefined);
  }
  assert.equal(stored(storage), before);
});

test("an open conversation's latest page is restored through the same parser, marked as saved", async () => {
  const storage = browserStore();
  mockResponse({ name: "design assistant", run_id: "run/one", running: true, answerable: true, interactive: true, caps: { rewind: true }, cwd: "/work", output: "", total_turns: 40, state: "running",
    pending_question: { id: "ask-1", note: "", headline: "Which one?", asked_at: 5, deadline: null, questions: [{ header: "Pick", question: "Which?", multi_select: false, options: [{ label: "A", description: "First" }] }] },
    awaits: [{ id: "w-1", note: "build", summary: "waiting", check: "true" }], stats: { failed: [{ turn: 39, step: 1 }], by_tool: [{ tool: "Bash", calls: 4, failed: 1 }] },
    turns: [
      { role: "user", text: "Hello", seq: 38, at: 10, images: [{ id: "img-1", name: "a.png", media_type: "image/png", size: 12 }], markers: [{ kind: "from", node: "studio", user: "igor" }] },
      { role: "assistant", text: "Hi", seq: 39, at: 11, pending: true, steps: [{ kind: "thinking", text: "hm" }, { kind: "tool", name: "Bash", input: "ls", status: "error", output: "no" },
        { kind: "calls", from: 2, to: 5, count: 4, tools: ["Bash"], preview: "ls", status: "ok" }, { kind: "future-step" }] },
      { role: "user", text: "Later", seq: 40, at: 12, queued: true, mode: "asap" },
    ] });
  const live = await getConversation(ref);
  assert.equal(live.saved, undefined);
  const restored = savedConversation(ref, {}, storage);
  assert.deepEqual(restored?.data, { ...live, saved: true });
  assert.equal(savedConversation({ ...ref, source: "studio" }, {}, storage), undefined, "the same run on another machine is another conversation");
  assert.equal(savedConversation({ ...ref, runId: "run/two" }, {}, storage), undefined);
  // Earlier and longer pages are neither stored nor restored.
  const before = stored(storage);
  await getConversation(ref, undefined, { limit: 40 });
  await getConversation(ref, undefined, { before: 38 });
  assert.equal(stored(storage), before);
  assert.equal(savedConversation(ref, { limit: 40 }, storage), undefined);
  assert.equal(savedConversation(ref, { before: 38 }, storage), undefined);
});

test("only the conversations opened most recently keep a saved page", async () => {
  const storage = browserStore();
  const realNow = Date.now;
  try {
    for (let index = 0; index < 10; index++) {
      Date.now = () => 1_000_000 + index;
      mockResponse({ name: "designer", run_id: `run-${index}`, turns: [{ role: "user", text: `message ${index}`, seq: 0 }] });
      await getConversation({ agent: "designer", runId: `run-${index}` });
    }
  } finally { Date.now = realNow; }
  const kept = [...Array(10).keys()].filter((index) => savedConversation({ agent: "designer", runId: `run-${index}` }, {}, storage));
  assert.deepEqual(kept, [2, 3, 4, 5, 6, 7, 8, 9]);
});

test("a read that fails leaves the last good saved copy alone", async () => {
  const storage = browserStore();
  mockResponse({ agents: [{ name: "designer" }] });
  await getAgents();
  mockResponse({ ok: false, error: "The machine is restarting." }, 503);
  await assert.rejects(() => getAgents(), /restarting/);
  mockResponse({ agents: [{ running: true }] });
  await assert.rejects(() => getAgents(), /invalid response/);
  assert.deepEqual(savedAgents(undefined, storage)?.data.map((agent) => agent.name), ["designer"]);
});

test("a stored value in an unknown shape is dropped instead of reaching the page", () => {
  const storage = browserStore();
  storage.setItem("adi.chat.fleet.v1", JSON.stringify({ savedAt: 1, payload: { nodes: [{ locked: true }] } }));
  storage.setItem("adi.chat.agents.v1:", JSON.stringify({ savedAt: 1, payload: [{ name: "designer" }] }));
  storage.setItem(`adi.chat.index.v1:${JSON.stringify(["", "me", "flat"])}`, JSON.stringify({ savedAt: 1, payload: { agents: [{ name: "designer", runs: [{ title: "No id" }] }] } }));
  storage.setItem(`adi.chat.thread.v1:${JSON.stringify(["", ref.agent, ref.runId])}`, JSON.stringify({ savedAt: 1, payload: { turns: [{ images: [{ name: "no id" }] }] } }));
  storage.setItem("adi.chat.voice.v1", JSON.stringify({ savedAt: 1, payload: { engines: "browser" } }));
  assert.equal(savedFleetNodes(storage), undefined);
  assert.equal(savedAgents(undefined, storage), undefined);
  assert.equal(savedConversationIndex(undefined, { filter: "me" }, storage), undefined);
  assert.equal(savedConversation(ref, {}, storage), undefined);
  assert.equal(savedVoice(storage), undefined);
});
