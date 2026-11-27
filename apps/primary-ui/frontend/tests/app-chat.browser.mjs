/**
 * End-to-end checks of the live React app chat, using isolated Chrome and intercepted API fixtures.
 * Every /api/app/* request is fulfilled locally; this script never forwards a chat mutation.
 * Run: bun frontend/tests/app-chat.browser.mjs
 * Optional: SHOWCASE_URL=http://design-system.adi CHROME_BIN=/path/to/chrome
 * Optional: BROWSER_SCREENSHOTS_DIR=/tmp/chat-screenshots, MOBILE_ONLY=1
 */
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

const chromePath = process.env.CHROME_BIN ?? (process.platform === "darwin"
  ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : "google-chrome");
const origin = new URL(process.env.SHOWCASE_URL ?? "http://design-system.adi").origin;
const apiBase = process.env.APP_API_BASE ?? "/api/app";
const fixturePath = (pathname) => apiBase === "/api" && pathname.startsWith("/api/") ? `/api/app${pathname.slice(4)}` : pathname;
const profile = await mkdtemp(resolve(tmpdir(), "design-system-app-chat-tests-"));
const chrome = Bun.spawn([
  chromePath, "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`,
  "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "about:blank",
], { stdout: "ignore", stderr: "ignore" });

const stars = new Set();
const main = { agent: "design-assistant", runId: "design-main" };
const other = { agent: "design-assistant", runId: "design-other" };
const child = { agent: "layout-reviewer", runId: "review-run" };
const question = { agent: "design-assistant", runId: "question-run" };
const markdown = { agent: "design-assistant", runId: "markdown-run" };
const compact = { agent: "design-assistant", runId: "compact-run" };
const compactText = "A long message should keep a clear gap between the speaker and the opposite edge of the transcript. ".repeat(9);
const markdownCode = `const html = "<section>escaped & preserved</section>";\n  const indented = true;  \nconst longLine = "${"code-width-".repeat(25)}";\n`;
const markdownOneLiner = `curl --data "${"one-line-width-".repeat(20)}" https://example.com/submit`;
const markdownOutput = "**tool output stays literal**\n<h2>tool markup stays literal</h2>";
const typescriptCell = 'const rows: string[] = (await sh("git ls-files", { check: false })).lines; // every tracked file\nfor (const row of rows.slice(0, 3)) console.log(`<b>${row}</b>`);\nrows.length;';
const pythonCell = 'rows = sh("git ls-files", check=False).lines  # every tracked file\nfor row in rows[:3]:\n    print(f"<b>{row}</b>")\nlen(rows)';
const markdownText = [
  "## Markdown fixture",
  "",
  "A **bold phrase**, an *emphasized phrase*, and `inline-code`.",
  "",
  "- Parent item",
  "  - Nested item",
  "- Another item",
  "",
  "1. First ordered item",
  "2. Second ordered item",
  "",
  "> A quoted paragraph",
  "",
  "| Name | Details |",
  "| --- | --- |",
  `| Wide content | ${"table-width-".repeat(30)} |`,
  "",
  "- [x] Completed task",
  "- [ ] Pending task",
  "",
  "First hard-break line",
  "Second hard-break line",
  "",
  "[Safe documentation](https://example.com/docs?view=chat)",
  "",
  "[Unsafe link](javascript:alert%281%29)",
  "",
  '<img src="invalid-markdown-image" onerror="window.__markdownExecuted = true">',
  "",
  '<script>window.__markdownExecuted = true</script>',
  "",
  "```js",
  markdownCode.trimEnd(),
  "```",
  "",
  "```sh",
  markdownOneLiner,
  "```",
].join("\n");
const capabilities = { answerable: true, interactive: false, history: true, images: false, tool_steps: true, batches: true };
const snapshots = new Map();
const titles = new Map();
const requests = [];
const errors = [];
const mockErrors = [];
const checks = [];
let socket;
let send;
let evaluate;
let failAfterAccept = false;
let failNextReply = false;
let holdNextPeek = null;
let heldPeek = null;
let holdNextReply = false;
let heldReply = null;
let holdNextStart = false;
let heldStart = null;
let created = 0;
let failNextStar = false;
let fireAwaitOnCancel = false;
let failNextRename = false;
let holdNextEdit = false;
let heldEdit = null;
const hiddenRuns = new Set();
let failNextHide = false;
let holdIndexRefresh = false;
let failNextIndex = false;
let failEveryIndex = false;
const heldIndexes = [];
async function releaseIndexRefresh() {
  holdIndexRefresh = false;
  await Promise.all(heldIndexes.splice(0).map((release) => release()));
}
let failTranscription = false;
// The reads a chat page opens with. Holding them shows what the page paints before any of them
// has answered, which on a return visit is what this browser saved the last time.
const openingRead = (method, path) => method === "GET" ? ["/api/app/fleet/nodes", "/api/app/agents", "/api/app/agents/runs/all", "/api/app/voice"].includes(path)
  : method === "POST" && path === "/api/app/agents/run/peek";
let heldReads = null;
function holdReads() {
  let release;
  heldReads = new Promise((resolve) => { release = resolve; });
  return () => { heldReads = null; release(); };
}
/** A conversation's last activity, when a scenario needs a different one to be the newest. */
const activity = new Map();
/** The unfolded steps of a turn that is still being written, when a scenario grows one: `{ turn, steps }` by conversation. */
const liveSteps = new Map();
let failLiveSteps = false;
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const idOf = (ref) => `${ref.agent}:${ref.runId}`;
const copy = (value) => structuredClone(value);
const conversationUrl = (ref) => `${origin}/interface?${new URLSearchParams({ agent: ref.agent, run: ref.runId })}`;
const listLink = (ref) => `[data-slot="app-conversation-link"][data-agent="${ref.agent}"][data-run="${ref.runId}"]`;
const composer = '[data-slot="app-chat"] [data-slot="chat-composer-input"]';
const chat = '[data-slot="app-chat"]';
const q = (selector) => `document.querySelector(${JSON.stringify(selector)})`;

function user(text, seq) {
  return { role: "user", text, seq, at: 1000 + seq, pending: false, queued: false, images: [], steps: [] };
}
function assistant(text, seq, steps = []) {
  return { role: "assistant", text, seq, at: 1000 + seq, pending: false, queued: false, images: [], steps };
}
function put(ref, title, turns, extras = {}) {
  titles.set(idOf(ref), title);
  snapshots.set(idOf(ref), {
    name: ref.agent, run_id: ref.runId, running: false, answerable: true, interactive: false,
    caps: capabilities, cwd: "/fixture/workspace", output: "", turns, total_turns: turns.length,
    awaits: [], ...extras,
  });
}
put(main, "Design the chat", [user("Mock main request", 0), assistant("Mock main answer", 1, [
  { kind: "calls", from: 0, to: 1, count: 1, tools: ["functions.exec_command"], preview: "/bin/zsh -lc 'git status --short'", status: "ok" },
  { kind: "message", text: "Mock delegated check" },
  { kind: "tool", name: "Bash", input: JSON.stringify({ command: "adi-mono agents run layout-reviewer 'Check the chat layout'" }), status: "ok", output: "Started agent layout-reviewer as background process 1234.\n  run: review-run\n" },
])]);
put(other, "Another conversation", [user("Mock other request", 0), assistant("Mock other answer", 1)]);
put(child, "Check the chat layout", [user("Mock review request", 0), assistant("Mock child answer", 1)]);
put(question, "A decision is needed", [user("Mock question request", 0), assistant("Mock question context", 1)], {
  pending_question: {
    id: "fixture-ask-17", asked_at: 1000, note: "Choose the label", headline: "Which label?", deadline: null,
    questions: [{ header: "Label", question: "Which label should be used?", options: [{ label: "Compact", description: "Short label" }], multi_select: false }],
  },
});
put(markdown, "Markdown rendering", [
  user("A **user bold phrase** with `user-code`.", 0),
  assistant(markdownText, 1, [
    { kind: "message", text: "### Interim checkpoint\n\nAn **interim bold phrase**." },
    { kind: "tool", name: "Bash", input: JSON.stringify({ command: "printf markdown-fixture" }), status: "ok", output: markdownOutput },
  ]),
  // A turn of its own, so the cell is not folded into a group with the command above it.
  user("Python fixture request", 2),
  assistant("Python fixture answer", 3, [
    { kind: "tool", name: "mcp__adi__PyCode", input: JSON.stringify({ code: pythonCell, timeout_ms: 30000 }), status: "ok", output: "3" },
  ]),
  user("TypeScript fixture request", 4),
  assistant("TypeScript fixture answer", 5, [
    { kind: "tool", name: "mcp__adi__TsCode", input: JSON.stringify({ code: typescriptCell }), status: "ok", output: "3" },
  ]),
]);

put(compact, "Compact messages and tool errors", [
  user(`User layout fixture. ${compactText}`, 0),
  assistant(`Assistant layout fixture. ${compactText}`, 1, [
    { kind: "calls", from: 0, to: 8, count: 8, tools: ["mcp__codex__exec_command", "functions.apply_patch", "functions.exec_command"], preview: "Check the workspace", status: "ok" },
  ]),
], {
  stats: {
    failed: [{ turn: 1, step: 0, run_from: 0, tool: "Bash", arg: "git missing-fixture-command" }],
    by_tool: [{ tool: "Bash", calls: 8, failed: 1 }],
  },
});

function allRuns() {
  const agents = ["design-assistant", "layout-reviewer"].map((name) => ({
    name, answerable: true, interactive: false, caps: capabilities,
    runs: [...snapshots.values()].filter((snapshot) => snapshot.name === name && !hiddenRuns.has(`${name}:${snapshot.run_id}`)).map((snapshot, index) => ({
      run_id: snapshot.run_id, caps: capabilities, started_at: 5000 - index * 100,
      last_activity: activity.get(`${name}:${snapshot.run_id}`) ?? 5000 - index * 100, message: titles.get(`${name}:${snapshot.run_id}`),
      title: titles.get(`${name}:${snapshot.run_id}`), running: snapshot.running, hidden: false, starred: stars.has(`${name}:${snapshot.run_id}`), launched_by: "human",
      ...(snapshot.pending_question ? { pending_question: snapshot.pending_question } : {}),
    })),
  }));
  return { agents, total: [...snapshots.values()].length };
}

async function until(fn, description, timeout = 7000) {
  const deadline = Date.now() + timeout;
  let last;
  do {
    try {
      const result = await fn();
      if (result) return result;
    } catch (error) { last = error; }
    await pause(25);
  } while (Date.now() < deadline);
  throw new Error(`Timed out: ${description}${last ? ` (${last.message})` : ""}`);
}

async function fulfill(requestId, data) {
  try {
    await send("Fetch.fulfillRequest", {
      requestId, responseCode: 200,
      responseHeaders: [{ name: "Content-Type", value: "application/json" }, { name: "Cache-Control", value: "no-store" }],
      body: Buffer.from(JSON.stringify(data)).toString("base64"),
    });
  } catch (error) {
    // A navigation may cancel an intentionally held read before its fixture is released.
    if (!/Invalid InterceptionId|Invalid interceptionId|Invalid requestId|No resource with given identifier|interception.*not found/i.test(error.message)) throw error;
  }
}

async function intercept({ requestId, request }) {
  try {
    const url = new URL(request.url);
    const path = fixturePath(url.pathname);
    const body = request.postData && path !== "/api/app/voice/transcribe" ? JSON.parse(request.postData) : {};
    requests.push({ method: request.method, path, query: url.search, body });
    // A held read is answered with whatever the fixtures say once it is released.
    if (heldReads && openingRead(request.method, path)) await heldReads;
    if (!path.startsWith("/api/app/")) {
      mockErrors.push(`Unexpected direct API request: ${request.method} ${path}`);
      await fulfill(requestId, { ok: false, error: "Direct API requests are disabled in this browser fixture." });
      return;
    }
    if (request.method === "POST" && path === "/api/app/voice/transcribe") {
      if (failTranscription) { failTranscription = false; await fulfill(requestId, {ok:false,error:"Fixture transcription failed"}); return; }
      await fulfill(requestId, {text:"Recorded fixture words",engine:"openai"}); return;
    }
    if (request.method === "GET" && path === "/api/app/voice") {
      await fulfill(requestId, { engines: [{id:"browser", label:"Browser", ready:true, in_browser:true, detail:"Browser recognition"}, {id:"openai",label:"OpenAI",ready:true,in_browser:false,detail:"Fixture"}], default_engine:"browser" }); return;
    }
    if (request.method === "GET" && path === "/api/app/marketplace") {
      await fulfill(requestId, {sources: [], apps: []});
      return;
    }
    if (request.method === "GET" && path === "/api/app/dashboards") {
      await fulfill(requestId, { dashboards: [{ id: "design-system", name: "Design System", description: "Shared components", host: "design-system.adi", frontend_running: true, backend_running: true, icon: "blocks", archived_at: null }] });
      return;
    }
    if (request.method === "GET" && path === "/api/app/fleet/dashboards") {
      await fulfill(requestId, { nodes: [] });
      return;
    }
    if (request.method === "GET" && path === "/api/app/fleet/nodes") {
      await fulfill(requestId, { nodes: [] });
      return;
    }
    if (request.method === "GET" && path === "/api/app/agents") {
      await fulfill(requestId, { agents: ["design-assistant", "layout-reviewer"].map((name) => ({ name, starred: name === "design-assistant", running: false, runnable: true, backend: "process:fixture", caps: capabilities })) });
      return;
    }
    if (request.method === "GET" && path === "/api/app/agents/runs/all") {
      assert.equal(url.searchParams.get("hidden"), "false");
      if (failNextIndex || failEveryIndex) { failNextIndex = false; await fulfill(requestId, { ok: false, error: "Fixture index refused" }); return; }
      if (holdIndexRefresh) { heldIndexes.push(() => fulfill(requestId, allRuns())); return; }
      await fulfill(requestId, allRuns());
      return;
    }
    if (request.method === "POST" && /\/agents\/run\/(hide|star|rename)$/.test(path) && holdNextEdit) {
      holdNextEdit = false;
      await new Promise((resolve) => { heldEdit = resolve; });
      heldEdit = null;
    }
    if (request.method === "POST" && path === "/api/app/agents/run/hide") {
      if (failNextHide) { failNextHide = false; await fulfill(requestId, {ok:false,error:"Fixture hide refused"}); return; }
      assert.equal(body.hidden, true);
      hiddenRuns.add(`${body.name}:${body.run_id}`);
      await fulfill(requestId, {ok:true}); return;
    }
    if (request.method === "POST" && path === "/api/app/agents/run/star") {
      if (failNextStar) { failNextStar = false; await fulfill(requestId, {ok:false,error:"Fixture star refused"}); return; }
      const id = `${body.name}:${body.run_id}`;
      if (body.starred) stars.add(id); else stars.delete(id);
      await fulfill(requestId, { ok: true }); return;
    }
    if (request.method === "POST" && path === "/api/app/agents/run/rename") {
      if (failNextRename) { failNextRename=false; await fulfill(requestId,{ok:false,error:"Fixture rename refused"}); return; }
      titles.set(`${body.name}:${body.run_id}`, body.title || "Default fixture title");
      await fulfill(requestId, { agents: [] }); return;
    }
    const ref = { agent: body.name, runId: body.run_id };
    const snapshot = snapshots.get(idOf(ref));
    if (request.method === "POST" && path === "/api/app/agents/run/peek" && snapshot) {
      const response = copy(snapshot);
      if (holdNextPeek === idOf(ref)) {
        holdNextPeek = null;
        heldPeek = { release: () => fulfill(requestId, response) };
        return;
      }
      await fulfill(requestId, response);
      return;
    }
    const live = liveSteps.get(idOf(ref));
    if (request.method === "POST" && path === "/api/app/agents/run/steps" && live?.turn === body.turn) {
      if (failLiveSteps) { await fulfill(requestId, { ok: false, error: "Fixture steps refused" }); return; }
      // Clamped as the server clamps it: a header drawn before the run grew names a shorter range.
      await fulfill(requestId, { name: ref.agent, run_id: ref.runId, turn: body.turn, from: body.from, steps: copy(live.steps.slice(body.from, body.to || undefined)) });
      return;
    }
    if (request.method === "POST" && path === "/api/app/agents/run/steps" && snapshot) {
      await fulfill(requestId, { name: ref.agent, run_id: ref.runId, turn: body.turn, from: body.from,
        steps: ref.runId === compact.runId
          ? Array.from({ length: 8 }, (_, index) => ({ kind: "tool", name: index === 1 ? "functions.apply_patch" : "mcp__codex__exec_command", input: JSON.stringify(index === 1 ? { patch: "*** Begin Patch\n*** Update File: /fixture/workspace/chat.tsx\n*** End Patch" } : { cmd: index === 0 ? "git missing-fixture-command" : `printf fixture-step-${index}` }), status: index === 0 ? "error" : "ok", output: index === 0 ? "Mock command failed" : "Mock command completed" }))
          : [{ kind: "tool", name: "functions.exec_command", input: JSON.stringify({ cmd: "/bin/zsh -lc 'git status --short'" }), status: "ok", output: "Mock clean worktree" }],
      });
      return;
    }
    if (request.method === "POST" && path === "/api/app/agents/run/reply" && snapshot) {
      if (failNextReply) {
        failNextReply = false;
        await fulfill(requestId, { ok: false, error: "Fixture send was refused. Please try again." });
        return;
      }
      const next = user(body.message, snapshot.turns.length);
      next.queued = snapshot.running;
      next.mode = body.mode;
      snapshot.turns.push(next);
      if (!snapshot.running) snapshot.turns.push(assistant("Mock reply accepted", snapshot.turns.length));
      snapshot.total_turns = snapshot.turns.length;
      if (failAfterAccept) {
        failAfterAccept = false;
        await fulfill(requestId, { ok: false, error: "Snapshot failed after accepting the message" });
        return;
      }
      if (holdNextReply) {
        holdNextReply = false;
        const response = copy(snapshot);
        heldReply = { release: () => fulfill(requestId, response) };
        return;
      }
      await fulfill(requestId, copy(snapshot));
      return;
    }
    if (request.method === "POST" && path === "/api/app/agents/run") {
      const ref = { agent: body.name, runId: `fixture-created-${++created}` };
      put(ref, "A new fixture conversation", [user(body.message, 0), assistant("Mock new chat answer", 1)]);
      const response = { run_id: ref.runId, message: "Started fixture", state: { agents: [] } };
      if (holdNextStart) {
        holdNextStart = false;
        heldStart = { release: () => fulfill(requestId, response) };
        return;
      }
      await fulfill(requestId, response);
      return;
    }
    if (request.method === "POST" && path === "/api/app/agents/run/stop" && snapshot) {
      snapshot.running = false;
      snapshot.turns.forEach((turn) => { turn.pending = false; });
      await fulfill(requestId, { name: ref.agent, runs: [] });
      return;
    }
    if (request.method === "POST" && path === "/api/app/agents/run/unqueue" && snapshot) {
      const dropped = snapshot.turns.filter((turn) => turn.queued)[body.index];
      if (dropped) snapshot.turns.splice(snapshot.turns.indexOf(dropped), 1);
      snapshot.total_turns = snapshot.turns.length;
      await fulfill(requestId, copy(snapshot));
      return;
    }
    if (request.method === "POST" && path === "/api/app/agents/await/ignore" && snapshot) {
      const pending = !fireAwaitOnCancel && snapshot.awaits.some((item) => item.id === body.id);
      fireAwaitOnCancel = false;
      snapshot.awaits = snapshot.awaits.filter((item) => item.id !== body.id);
      await fulfill(requestId, pending ? { awaits: copy(snapshot.awaits) }
        : { ok: false, error: `await ${body.id} changed or fired before this reached it` });
      return;
    }
    if (request.method === "POST" && path === "/api/app/agents/run/answer" && snapshot) {
      assert.equal(body.ask, snapshot.pending_question?.id, "answer identifies the current ask");
      delete snapshot.pending_question;
      snapshot.turns.push(user("Mock decision recorded", snapshot.turns.length));
      snapshot.total_turns = snapshot.turns.length;
      await fulfill(requestId, copy(snapshot));
      return;
    }
    // The rail's event log keeps its rows on the machine; this fixture has none stored, so the
    // panel shows exactly what the page reported.
    if (request.method === "POST" && /^\/api\/app\/diagnostics\/(query|report)$/.test(path)) {
      await fulfill(requestId, path.endsWith("/query") ? { records: [] } : { ok: true });
      return;
    }
    mockErrors.push(`Unimplemented intercepted API route: ${request.method} ${path}`);
    await fulfill(requestId, { ok: false, error: "This API route is unavailable in the browser fixture." });
  } catch (error) {
    mockErrors.push(error.message);
    await fulfill(requestId, { ok: false, error: "The browser fixture could not handle this request." });
  }
}

try {
  const port = await until(async () => Number((await readFile(resolve(profile, "DevToolsActivePort"), "utf8")).split("\n")[0]), "Chrome debugging port");
  const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  socket = new WebSocket(tabs.find((tab) => tab.type === "page").webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let nextId = 1;
  const pending = new Map();
  send = (method, params = {}) => {
    const id = nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 10000);
      pending.set(id, { resolve, reject, timer });
      socket.send(JSON.stringify({ id, method, params }));
    });
  };
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id) {
      const waiting = pending.get(message.id);
      if (!waiting) return;
      pending.delete(message.id);
      clearTimeout(waiting.timer);
      message.error ? waiting.reject(new Error(JSON.stringify(message.error))) : waiting.resolve(message.result);
    }
    if (message.method === "Fetch.requestPaused") void intercept(message.params).catch((error) => mockErrors.push(error.message));
    if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text);
    if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") errors.push(message.params.args.map((item) => item.value ?? item.description).join(" "));
    // A failed resource load says nothing without the address it failed for.
    if (message.method === "Log.entryAdded" && message.params.entry.level === "error") errors.push(message.params.entry.url ? `${message.params.entry.text} (${message.params.entry.url})` : message.params.entry.text);
  };
  evaluate = async (expression) => {
    const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    return result.result.value;
  };
  async function click(selector) {
    await evaluate(`${q(selector)}.scrollIntoView({block:'center'})`);
    const point = await evaluate(`(() => { const r=${q(selector)}.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
    await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, ...point });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, ...point });
  }
  async function key(key) {
    const code = { Enter: 13, Escape: 27, Backspace: 8 }[key];
    await send("Input.dispatchKeyEvent", { type: "keyDown", key, code: key, windowsVirtualKeyCode: code, ...(key === "Enter" ? { text: "\r" } : {}) });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key, code: key, windowsVirtualKeyCode: code });
  }
  async function type(selector, text) {
    await click(selector);
    await evaluate(`${q(selector)}.select()`);
    await key("Backspace");
    if (text) await send("Input.insertText", { text });
    // Input.insertText acknowledges the native event before React necessarily commits its controls.
    await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  }
  const draft = () => evaluate(`${q(composer)}?.value`);
  const contains = (text) => evaluate(`${q(`${chat} [data-slot="chat-thread"]`)}?.textContent.includes(${JSON.stringify(text)}) === true`);
  const activeIs = (ref) => evaluate(`(() => { const p=new URLSearchParams(location.search); return p.get('agent')===${JSON.stringify(ref.agent)} && p.get('run')===${JSON.stringify(ref.runId)}; })()`);
  const mutations = (suffix) => requests.filter((request) => request.method === "POST" && request.path === `/api/app/agents/${suffix}`);
  async function open(ref, text) {
    if (await evaluate(`innerWidth < 768 && !!document.querySelector('[data-mobile-screen="conversation"]')`)) {
      await click('[aria-label="Back to chats"]');
      await until(() => evaluate(`!!document.querySelector('[data-mobile-screen="chats"]')`), "mobile inbox");
    }
    await click(listLink(ref));
    await until(() => activeIs(ref), "selected conversation route");
    if (text) await until(() => contains(text), "selected conversation transcript");
  }
  async function check(name, fn) {
    if (process.env.MOBILE_ONLY && !name.startsWith("mobile ")) return;
    await fn();
    checks.push(name);
    console.log(`PASS ${name}`);
  }
  async function setTheme(dark) {
    const toggle = '[data-slot="theme-toggle"]';
    if (await evaluate(`${q(toggle)}.getAttribute('aria-checked')`) !== String(dark)) await click(toggle);
    await until(() => evaluate(`document.documentElement.classList.contains('dark')===${dark}&&${q(toggle)}.getAttribute('aria-checked')==='${dark}'`), "global theme toggle applies its selected theme");
  }
  async function sidebarStatus(ref, label) {
    return until(() => evaluate(`(() => {
      const link=${q(listLink(ref))};
      if(link?.querySelector('.sr-only')?.textContent!==${JSON.stringify(label)})return null;
      const dot=link.querySelector('span[aria-hidden="true"]');
      if(!dot)return null;
      const context=document.createElement('canvas').getContext('2d', {willReadFrequently:true});
      context.fillStyle=getComputedStyle(dot).backgroundColor;
      context.fillRect(0,0,1,1);
      return [...context.getImageData(0,0,1,1).data];
    })()`), `sidebar status is ${label}`);
  }

  await Promise.all([send("Runtime.enable"), send("Page.enable"), send("Log.enable")]);
  // This is enabled before navigating. There is deliberately no continueRequest path for API calls.
  await send("Fetch.enable", { patterns: [
    { urlPattern: "*/api/*", requestStage: "Request" },
    { urlPattern: "*://app.adi/api/*", requestStage: "Request" },
  ] });
  await send("Page.addScriptToEvaluateOnNewDocument", { source: `
    Object.defineProperty(window,'isSecureContext',{value:true,configurable:true});
    window.__tracksStopped=0;
    window.SpeechRecognition=class {
      start(){window.__recognition=this;}
      stop(){this.onend?.();}
      abort(){this.onend?.();window.__recognition=null;}
    };
    Object.defineProperty(navigator,'mediaDevices',{value:{getUserMedia:async()=>({getTracks:()=>[{stop:()=>window.__tracksStopped++}]})}});
    window.MediaRecorder=class {
      static isTypeSupported(){return true;}
      constructor(stream,options){this.mimeType=options?.mimeType||'audio/webm';this.state='inactive';}
      start(){this.state='recording';}
      stop(){this.state='inactive';this.ondataavailable?.({data:new Blob(['fixture-audio'],{type:this.mimeType})});this.onstop?.();}
    };
    // Every document notes each distinct state the chat page was painted in, from the first frame on.
    window.chatFrames = [];
    (function sample() {
      const side = document.querySelector('[data-slot="app-chat-sidebar"]');
      const main = document.querySelector('[data-slot="app-chat"]');
      if (side || main) {
        const thread = main?.querySelector('[data-slot="chat-thread"]')?.textContent ?? '';
        const frame = {
          rows: side ? side.querySelectorAll('[data-slot="app-conversation-link"]').length : null,
          loading: [...document.querySelectorAll('[data-slot="app-chat-sidebar"] [role="status"], [data-slot="app-chat"] [role="status"]')].filter((el) => el.checkVisibility()).map((el) => el.textContent).filter((text) => text.startsWith('Loading')).join('|'),
          run: new URLSearchParams(location.search).get('run'),
          title: main ? main.querySelector('header h1')?.textContent ?? '' : null,
          answers: ['Mock main answer', 'Mock other answer', 'Mock caught-up answer'].filter((text) => thread.includes(text)).join('|'),
          newChat: !!main?.querySelector('[data-slot="app-chat-agent"]'),
          composerOff: main ? main.querySelector('textarea')?.disabled ?? null : null,
        };
        const last = window.chatFrames[window.chatFrames.length - 1];
        if (!last || Object.keys(frame).some((key) => last[key] !== frame[key])) window.chatFrames.push(frame);
      }
      requestAnimationFrame(sample);
    })();
  ` });
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
  await send("Page.navigate", { url: conversationUrl(main) });
  await until(() => contains("Mock main answer"), "connected chat renders fixtures", 15000);
  await setTheme(false);

  await check("conversation sidebar lists, searches and navigates the actual API contract", async () => {
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="app-conversation-link"]').length`), 5);
    await click('[data-slot="app-conversation-group-more"]');
    await until(() => evaluate(`document.querySelectorAll('[data-slot="app-conversation-link"]').length===6`), "machine pagination reveals remaining chat");
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="app-conversation-link"]').length`), 6);
    assert.equal(await evaluate(`${q(listLink(main))}.getAttribute('aria-current')`), "page");
    assert.equal(await evaluate(`${q(listLink(other))}.getAttribute('href')`), `/interface?${new URLSearchParams({ agent: other.agent, run: other.runId })}`);
    await type('[aria-label="Search conversations"]', "layout-reviewer");
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="app-conversation-link"]').length`), 1);
    await type('[aria-label="Search conversations"]', "does-not-exist");
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="app-conversation-link"]').length`), 0);
    await type('[aria-label="Search conversations"]', "");
    await open(other, "Mock other answer");
    assert.equal(await contains("Mock main answer"), false);
    await open(main, "Mock main answer");
  });

  await check("chat header copies the current thread ID over HTTP", async () => {
    await open(main, "Mock main answer");
    await evaluate(`window.__copiedChatIds = []; window.__originalCopyCommand = document.execCommand; document.execCommand = (command) => { if (command === 'copy') { window.__copiedChatIds.push(document.activeElement.value); return true; } return false; }`);
    try {
      await click('[aria-label="Copy chat ID"]');
      await until(() => evaluate(`document.querySelector('[role="status"]')?.textContent === 'Chat ID copied'`), "copy confirmation");
      assert.equal(await evaluate(`window.__copiedChatIds.at(-1)`), main.runId);
      await open(other, "Mock other answer");
      await click('[aria-label="Copy chat ID"]');
      await until(() => evaluate(`window.__copiedChatIds.at(-1) === ${JSON.stringify(other.runId)}`), "current chat ID copied");
      await evaluate(`document.execCommand = () => false`);
      await click('[aria-label="Copy chat ID"]');
      await until(() => evaluate(`document.querySelector('[role="alert"]')?.textContent.includes('Could not copy the chat ID')`), "copy failure is visible");
    } finally {
      await evaluate(`document.execCommand = window.__originalCopyCommand`);
    }
    await open(main, "Mock main answer");
  });

  await check("user, assistant and interim messages render Markdown and GFM semantics", async () => {
    await open(markdown, "Markdown fixture");
    const rendered = await evaluate(`(() => {
      const root = ${q(`${chat} [data-slot="chat-markdown"] h2`)}.closest('[data-slot="chat-markdown"]');
      const user = ${q(`${chat} [data-from="user"] [data-slot="chat-markdown"]`)};
      const interim = ${q(`${chat} [data-slot="chat-markdown"] h3`)}.closest('[data-slot="chat-markdown"]');
      return {
        heading: root.querySelector('h2')?.textContent,
        strong: root.querySelector('strong')?.textContent,
        emphasis: root.querySelector('em')?.textContent,
        inlineCode: [...root.querySelectorAll('code')].find(code => !code.closest('pre'))?.textContent,
        nestedList: root.querySelector('ul > li > ul > li')?.textContent,
        ordered: [...root.querySelectorAll('ol > li')].map(item => item.textContent),
        quote: root.querySelector('blockquote')?.textContent.trim(),
        table: { headers: [...root.querySelectorAll('table th')].map(cell => cell.textContent), rows: root.querySelectorAll('table tbody tr').length },
        tasks: [...root.querySelectorAll('input[type="checkbox"]')].map(input => ({checked: input.checked, disabled: input.disabled})),
        lineBreak: [...root.querySelectorAll('p')].some(paragraph => paragraph.textContent.includes('First hard-break line') && paragraph.querySelector('br')),
        code: root.querySelector('pre code')?.textContent,
        codeHasHtml: !!root.querySelector('pre section'),
        userBold: user?.querySelector('strong')?.textContent,
        userCode: user?.querySelector('code')?.textContent,
        interimHeading: interim.querySelector('h3')?.textContent,
        interimBold: interim.querySelector('strong')?.textContent,
        outerWhiteSpace: getComputedStyle(root.closest('[data-slot="chat-message-content"]')).whiteSpace,
      };
    })()`);
    assert.deepEqual(rendered, {
      heading: "Markdown fixture", strong: "bold phrase", emphasis: "emphasized phrase", inlineCode: "inline-code",
      nestedList: "Nested item", ordered: ["First ordered item", "Second ordered item"], quote: "A quoted paragraph",
      table: { headers: ["Name", "Details"], rows: 1 },
      tasks: [{ checked: true, disabled: true }, { checked: false, disabled: true }],
      lineBreak: true, code: markdownCode, codeHasHtml: false,
      userBold: "user bold phrase", userCode: "user-code", interimHeading: "Interim checkpoint", interimBold: "interim bold phrase",
      outerWhiteSpace: "normal",
    });
  });

  await check("Markdown links and raw HTML are safe while tool output remains literal", async () => {
    const links = await evaluate(`(() => {
      const root = ${q(`${chat} [data-slot="chat-markdown"] h2`)}.closest('[data-slot="chat-markdown"]');
      const safe = [...root.querySelectorAll('a')].find(link => link.textContent === 'Safe documentation');
      const unsafe = [...root.querySelectorAll('a')].find(link => link.textContent === 'Unsafe link');
      return { href: safe?.getAttribute('href'), target: safe?.target, rel: safe?.rel.split(/\\s+/), unsafeHref: unsafe?.getAttribute('href') ?? '', activeHtml: root.querySelectorAll('img,script,iframe').length, executed: window.__markdownExecuted === true };
    })()`);
    assert.equal(links.href, "https://example.com/docs?view=chat");
    assert.equal(links.target, "_blank");
    assert.ok(links.rel.includes("noopener") && links.rel.includes("noreferrer"));
    assert.equal(/^javascript:/i.test(links.unsafeHref), false);
    assert.equal(links.activeHtml, 0);
    assert.equal(links.executed, false);
    await click(`${chat} [data-slot="chat-tool-call-trigger"]`);
    const literal = await evaluate(`(() => { const output = ${q(`${chat} [data-slot="chat-tool-output"]`)}; return { text: output.textContent, markup: output.querySelectorAll('strong,h2').length, tag: output.tagName }; })()`);
    assert.equal(literal.text, markdownOutput);
    assert.equal(literal.markup, 0);
    assert.equal(literal.tag, "PRE");
    await click(`${chat} [data-slot="chat-tool-call-trigger"]`);
  });

  await check("a Python cell is shown as highlighted code above its output, with its options after it", async () => {
    await open(markdown, "Python fixture answer");
    const call = `${chat} [data-slot="chat-tool-call"][data-kind="code"]`;
    await click(`${call} [data-slot="chat-tool-call-trigger"]`);
    await until(() => evaluate(`${q(`${call} [data-slot="chat-tool-code"]`)}?.checkVisibility() === true`), "the Python cell opens");
    const shown = await evaluate(`(() => {
      const call = ${q(call)}, code = call.querySelector('[data-slot="chat-tool-code"]'), output = call.querySelector('[data-slot="chat-tool-output"]');
      const ink = (token) => getComputedStyle(code.querySelector('[data-token="' + token + '"]')).color;
      return {
        title: call.querySelector('[data-slot="chat-tool-call-title"]').textContent,
        language: code.dataset.language, label: code.getAttribute('aria-label'), text: code.textContent,
        tokens: [...new Set([...code.querySelectorAll('[data-token]')].map((span) => span.dataset.token))].sort(),
        // Markup inside the code is text, never elements.
        elements: code.querySelectorAll('b').length,
        inked: new Set([getComputedStyle(code).color, ink('kw'), ink('str'), ink('key')]).size,
        options: [...call.querySelectorAll('dl dt')].map((term) => term.textContent),
        output: output.textContent,
        codeFirst: Boolean(code.compareDocumentPosition(output) & Node.DOCUMENT_POSITION_FOLLOWING),
        contained: getComputedStyle(code).overscrollBehaviorX + '/' + getComputedStyle(code).overscrollBehaviorY,
      };
    })()`);
    assert.deepEqual(shown, {
      title: "PyCode", language: "py", label: "Python code", text: pythonCell,
      tokens: ["comment", "func", "key", "kw", "num", "punct", "str"],
      elements: 0, inked: 4, options: ["timeout_ms"], output: "3", codeFirst: true,
      // Sideways only: a vertical wheel turn over the cell still scrolls the transcript.
      contained: "contain/auto",
    });
    await click(`${call} [data-slot="chat-tool-call-trigger"]`);
  });

  await check("a TypeScript cell is shown the same way, read as TypeScript", async () => {
    await until(() => contains("TypeScript fixture answer"), "the TypeScript turn");
    const shown = await evaluate(`(() => {
      const call = [...document.querySelectorAll(${JSON.stringify(`${chat} [data-slot="chat-tool-call"][data-kind="code"]`)})].at(-1);
      call.querySelector('[data-slot="chat-tool-call-trigger"]').click();
      const code = call.querySelector('[data-slot="chat-tool-code"]');
      return {
        title: call.querySelector('[data-slot="chat-tool-call-title"]').textContent,
        language: code.dataset.language, label: code.getAttribute('aria-label'), text: code.textContent,
        keywords: [...new Set([...code.querySelectorAll('[data-token="kw"]')].map((span) => span.textContent))].sort(),
        elements: code.querySelectorAll('b').length,
        options: [...call.querySelectorAll('dl dt')].length,
      };
    })()`);
    assert.deepEqual(shown, {
      title: "TsCode", language: "ts", label: "TypeScript code", text: typescriptCell,
      keywords: ["await", "const", "false", "for", "of"], elements: 0, options: 0,
    });
  });

  await check("Markdown code and tables stay bounded in light, dark and narrow layouts", async () => {
    for (const mode of ["light", "dark", "mobile"]) {
      const mobile = mode === "mobile";
      await setTheme(mode === "dark");
      await send("Emulation.setDeviceMetricsOverride", { width: mobile ? 390 : 1280, height: 1000, deviceScaleFactor: 1, mobile });
      await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
      const bounds = await evaluate(`(() => {
        const root = ${q(`${chat} [data-slot="chat-markdown"] h2`)}.closest('[data-slot="chat-markdown"]');
        const bounded = element => {
          for (let parent = element; parent && parent !== root.parentElement; parent = parent.parentElement) {
            if (['auto', 'scroll'].includes(getComputedStyle(parent).overflowX) && parent.clientWidth <= root.clientWidth + 1) return true;
          }
          return false;
        };
        return { page: document.documentElement.scrollWidth <= innerWidth, code: bounded(root.querySelector('pre')), table: bounded(root.querySelector('table')), messageWidth: root.getBoundingClientRect().width, viewportWidth: innerWidth };
      })()`);
      assert.equal(bounds.page, true, `${mode} Markdown does not widen the page`);
      assert.equal(bounds.code, true, `${mode} code has a bounded horizontal scroll container`);
      assert.equal(bounds.table, true, `${mode} tables have a bounded horizontal scroll container`);
      assert.ok(bounds.messageWidth <= bounds.viewportWidth, `${mode} Markdown fits the viewport`);
      if (process.env.BROWSER_SCREENSHOTS_DIR) {
        await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
        await evaluate(`${q(`${chat} [data-slot="chat-thread-viewport"]`)}.scrollTo({top:0,behavior:'instant'})`);
        await pause(150);
        const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
        await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, `design-system-app-markdown-${mode}.png`), Buffer.from(screenshot.data, "base64"));
      }
    }
    await setTheme(false);
    await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
  });

  await check("a code block copies its code from a button pinned to its top-right corner", async () => {
    const frame = `${chat} [data-slot="chat-markdown-code"]`;
    const pre = `${frame} pre`;
    const button = `${frame} [data-slot="chat-markdown-code-copy"]`;
    await evaluate(`${q(frame)}.scrollIntoView({block:'center',behavior:'instant'})`);
    await pause(100);
    const place = () => evaluate(`(() => {
      const b=${q(button)}.getBoundingClientRect(), p=${q(pre)}.getBoundingClientRect();
      return { top: b.top - p.top, right: p.right - b.right, inside: b.left >= p.left && b.bottom <= p.bottom, opacity: getComputedStyle(${q(button)}).opacity };
    })()`);
    const shown = (value, description) => until(async () => (await place()).opacity === value, description, 2500);
    const pointer = (x, y) => send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    const overCode = async () => { const r = await evaluate(`(() => { const r=${q(pre)}.getBoundingClientRect(); return {x:r.left+40,y:r.top+r.height/2}; })()`); await pointer(r.x, r.y); };

    await pointer(2, 2);
    await shown("0", "the copy button stays out of the way until the block is pointed at");
    await overCode();
    await shown("1", "pointing at a code block reveals its copy button");
    const corner = await place();
    assert.ok(corner.inside && corner.top >= 4 && corner.top <= 12 && corner.right >= 4 && corner.right <= 12, `the copy button sits in the block's top-right corner (${JSON.stringify(corner)})`);
    assert.equal(await evaluate(`${q(button)}.getAttribute('aria-label')`), "Copy code");
    if (process.env.BROWSER_SCREENSHOTS_DIR) {
      await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
      const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
      await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, "design-system-app-markdown-code-copy.png"), Buffer.from(screenshot.data, "base64"));
    }

    // The fixture's long line overflows; scrolling the code must not carry the button away.
    await evaluate(`${q(pre)}.scrollLeft = 300`);
    assert.ok(await evaluate(`${q(pre)}.scrollLeft`) > 100, "the fixture code scrolls sideways");
    const scrolled = await place();
    assert.deepEqual({ top: scrolled.top, right: scrolled.right }, { top: corner.top, right: corner.right }, "the copy button stays in the corner while the code scrolls");
    await evaluate(`${q(pre)}.scrollLeft = 0`);

    await evaluate(`window.__copiedCode = []; window.__originalCopyCommand = document.execCommand; document.execCommand = (command) => { if (command === 'copy') { window.__copiedCode.push(document.activeElement.value); return true; } return false; }`);
    try {
      await click(button);
      await until(() => evaluate(`${q(button)}.dataset.outcome === 'copied'`), "the button confirms the copy");
      // Exactly the code, inner spacing included, without the line break that closes the fence.
      assert.equal(await evaluate(`window.__copiedCode.at(-1)`), markdownCode.slice(0, -1));
      assert.equal(await evaluate(`${q(`${frame} [role="status"]`)}.textContent`), "Code copied");
      assert.equal(await evaluate(`document.activeElement === ${q(button)}`), true, "focus returns to the copy button");
      await until(() => evaluate(`${q(button)}.dataset.outcome === undefined`), "the confirmation clears", 4000);
      await evaluate(`document.execCommand = () => false`);
      await click(button);
      await until(() => evaluate(`${q(button)}.dataset.outcome === 'failed' && ${q(`${frame} [role="status"]`)}.textContent.includes('Could not copy the code')`), "a refused copy is reported");
    } finally {
      await evaluate(`document.execCommand = window.__originalCopyCommand`);
    }

    // Without a pointer over the block, keyboard focus on the code or on the button shows it too.
    await evaluate(`document.activeElement?.blur()`);
    await pointer(2, 2);
    await shown("0", "the copy button hides again once the block is left");
    await evaluate(`${q(pre)}.focus({preventScroll:true})`);
    await shown("1", "keyboard focus on a code block reveals its copy button");
    await evaluate(`document.activeElement?.blur()`);
    await shown("0", "the copy button hides again once focus leaves");

    // A touch screen has no hover: the button is always there, and a long first line scrolls clear of it.
    // Touch emulation is what turns the pointer media coarse; setEmulatedMedia does not carry that feature.
    await send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
    try {
      await shown("1", "a touch screen always shows the copy button");
      const clear = await evaluate(`(() => {
        const frame = document.querySelectorAll(${JSON.stringify(frame)})[1];
        const block = frame.querySelector('pre'), b = frame.querySelector('[data-slot="chat-markdown-code-copy"]').getBoundingClientRect(), p = block.getBoundingClientRect();
        const overflows = block.scrollWidth > block.clientWidth + 100;
        block.scrollLeft = block.scrollWidth;
        const range = document.createRange(); range.selectNodeContents(block.querySelector('code'));
        const ends = [...range.getClientRects()].filter(r => r.width > 0).map(r => r.right);
        block.scrollLeft = 0;
        return { overflows, lines: ends.length, end: Math.max(...ends), button: b.left, fits: b.top > p.top && b.bottom < p.bottom, code: block.textContent };
      })()`);
      assert.equal(clear.code, `${markdownOneLiner}\n`);
      assert.ok(clear.fits, "the copy button fits inside a one-line block");
      assert.ok(clear.overflows && clear.lines > 0 && clear.end <= clear.button, `scrolled to its end, a long line is not left under the button (${JSON.stringify({ ...clear, code: undefined })})`);
    } finally {
      await send("Emulation.setTouchEmulationEnabled", { enabled: false });
    }
  });

  await check("the transcript scrolls from its side margins and from over a table, code and tool output", async () => {
    // Wide enough for margins beside the messages, short enough for the transcript to overflow.
    await send("Emulation.setDeviceMetricsOverride", { width: 1600, height: 520, deviceScaleFactor: 1, mobile: false });
    await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
    const viewport = q(`${chat} [data-slot="chat-thread-viewport"]`);
    const box = (element) => evaluate(`(() => { const node=${element}; const style=getComputedStyle(node); const r=node.getBoundingClientRect(); return {left:r.left,right:r.right,top:r.top,bottom:r.bottom,width:r.width,innerLeft:r.left+parseFloat(style.paddingLeft),innerRight:r.right-parseFloat(style.paddingRight)}; })()`);
    const pane = await box(q(chat));
    const scroller = await box(viewport);
    const column = await box(q(`${chat} [data-slot="chat-thread-content"]`));
    const composerBox = await box(q(`${chat} [data-slot="chat-composer"]`));
    assert.ok(Math.abs(scroller.left - pane.left) <= 1 && Math.abs(scroller.right - pane.right) <= 1, "the scroll area spans the chat pane");
    assert.ok(column.width <= 896 + 1, "messages keep their reading width");
    assert.ok(column.left - scroller.left > 40 && scroller.right - column.right > 40, "the pane has margins beside the messages");
    assert.ok(Math.abs((column.left - scroller.left) - (scroller.right - column.right)) <= 1, "messages are centred in the pane");
    assert.ok(Math.abs(column.innerLeft - composerBox.left) <= 1 && Math.abs(column.innerRight - composerBox.right) <= 1, "messages line up with the composer");

    const scrollTop = () => evaluate(`${viewport}.scrollTop`);
    const room = await evaluate(`${viewport}.scrollHeight - ${viewport}.clientHeight`);
    assert.ok(room > 200, "the fixture transcript overflows its pane");
    /** A real wheel turn at a point; the transcript itself has to move. */
    async function wheelMovesTranscript(point, description) {
      const before = await scrollTop();
      const deltaY = before > room / 2 ? -120 : 120;
      await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: point.x, y: point.y, deltaX: 0, deltaY });
      await until(async () => Math.abs(await scrollTop() - before) > 20, description, 2500);
      // Let the wheel animation settle so the next turn starts from rest.
      await pause(350);
    }
    const middle = (scroller.top + scroller.bottom) / 2;
    await wheelMovesTranscript({ x: scroller.left + 12, y: middle }, "a wheel turn in the left margin scrolls the transcript");
    const jump = `${chat} [data-slot="chat-thread-jump"]`;
    await until(() => evaluate(`${q(jump)} !== null`), "scrolling away from the latest message offers a way back");
    assert.ok(Math.abs((await box(q(jump))).right - column.innerRight) <= 1, "the jump control sits at the message column's edge, not the pane's");
    if (process.env.BROWSER_SCREENSHOTS_DIR) {
      await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
      const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
      await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, "design-system-app-chat-wide-pane.png"), Buffer.from(screenshot.data, "base64"));
    }
    await wheelMovesTranscript({ x: scroller.right - 12, y: middle }, "a wheel turn in the right margin scrolls the transcript");

    async function over(selector) {
      await evaluate(`${q(selector)}.scrollIntoView({block:'center',behavior:'instant'})`);
      await pause(100);
      return evaluate(`(() => { const r=${q(selector)}.getBoundingClientRect(); return {x:r.left+Math.min(r.width/2,120),y:r.top+r.height/2}; })()`);
    }
    const table = `${chat} [data-slot="chat-markdown-table"]`;
    await wheelMovesTranscript(await over(table), "a wheel turn over a table scrolls the transcript");
    await wheelMovesTranscript(await over(`${chat} [data-slot="chat-markdown"] pre`), "a wheel turn over a short code block scrolls the transcript");
    await click(`${chat} [data-slot="chat-tool-call-trigger"]`);
    await until(() => evaluate(`${q(`${chat} [data-slot="chat-tool-output"]`)}?.checkVisibility() === true`), "tool output opens");
    await wheelMovesTranscript(await over(`${chat} [data-slot="chat-tool-output"]`), "a wheel turn over short tool output scrolls the transcript");
    await click(`${chat} [data-slot="chat-tool-call-trigger"]`);

    // A wide table still scrolls sideways inside its own box.
    const tablePoint = await over(table);
    assert.ok(await evaluate(`${q(table)}.scrollWidth > ${q(table)}.clientWidth + 100`), "the fixture table is wider than its message");
    await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: tablePoint.x, y: tablePoint.y, deltaX: 120, deltaY: 0 });
    await until(() => evaluate(`${q(table)}.scrollLeft > 20`), "a sideways wheel turn scrolls the table");
    await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
    await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  });

  await check("streamed Markdown tolerates incomplete fences and updates completed markup", async () => {
    const snapshot = snapshots.get(idOf(markdown));
    const streamed = { ...assistant('## Streaming markdown\n\n```ts\nconst partial = "<pending>";', snapshot.turns.length), pending: true };
    snapshot.running = true;
    snapshot.turns.push(streamed);
    snapshot.total_turns = snapshot.turns.length;
    await until(() => contains("Streaming markdown"), "incomplete streaming fence renders", 9000);
    assert.equal(await evaluate(`(() => { const root = [...document.querySelectorAll('${chat} [data-slot="chat-markdown"]')].find(message => message.querySelector('h2')?.textContent === 'Streaming markdown'); return root.querySelector('pre code').textContent; })()`), 'const partial = "<pending>";\n');
    streamed.text += "\nconst complete = true;\n```\n\n**Streaming complete**";
    streamed.pending = false;
    snapshot.running = false;
    await until(() => evaluate(`[...document.querySelectorAll('${chat} [data-slot="chat-markdown"] strong')].some(node => node.textContent === 'Streaming complete')`), "completed streaming Markdown updates", 9000);
    assert.deepEqual(await evaluate(`(() => { const root = [...document.querySelectorAll('${chat} [data-slot="chat-markdown"]')].find(message => message.querySelector('h2')?.textContent === 'Streaming markdown'); return {code:root.querySelector('pre code').textContent,pending:root.closest('[data-slot="chat-message"]').hasAttribute('data-pending')}; })()`), {
      code: 'const partial = "<pending>";\nconst complete = true;\n', pending: false,
    });
    await open(main, "Mock main answer");
  });

  await check("drafts remain attached to their own conversation", async () => {
    await type(composer, "Main draft retained");
    await open(other, "Mock other answer");
    assert.equal(await draft(), "");
    await type(composer, "Other draft retained");
    await open(main, "Mock main answer");
    assert.equal(await draft(), "Main draft retained");
    await open(other, "Mock other answer");
    assert.equal(await draft(), "Other draft retained");
    await open(main, "Mock main answer");
  });

  await check("obsolete conversation reads cannot replace the newly selected transcript", async () => {
    holdNextPeek = idOf(other);
    await click(listLink(other));
    await until(() => heldPeek !== null, "other transcript request is held");
    await open(main, "Mock main answer");
    await heldPeek.release();
    heldPeek = null;
    await pause(150);
    assert.equal(await activeIs(main), true);
    assert.equal(await contains("Mock other answer"), false);
    assert.equal(await draft(), "Main draft retained");
  });

  await check("Enter sends once, clears accepted drafts and retains refused drafts", async () => {
    let before = mutations("run/reply").length;
    await type(composer, "Accepted fixture reply");
    await key("Enter");
    await until(async () => mutations("run/reply").length === before + 1 && await draft() === "", "accepted reply clears its draft");
    assert.equal(mutations("run/reply").at(-1).body.run_id, main.runId);
    assert.equal(mutations("run/reply").at(-1).body.message, "Accepted fixture reply");
    await until(() => contains("Accepted fixture reply"), "accepted reply appears in the transcript");
    failNextReply = true;
    before = mutations("run/reply").length;
    await type(composer, "Refused fixture reply");
    await key("Enter");
    await until(() => evaluate(`${q(chat)}.querySelector('[role="alert"]') !== null`), "send failure is announced");
    assert.equal(await draft(), "Refused fixture reply");
    assert.equal(await contains("Refused fixture reply"), false, "refused text was not added to the transcript");
    await pause(100);
    assert.equal(mutations("run/reply").length, before + 1, "failed mutation is not automatically retried");
    await type(composer, "");
  });

  await check("an accepted message with a failed response is reconciled without duplicate sending", async () => {
    await open(main, "Mock main answer");
    const before = mutations("run/reply").length;
    failAfterAccept = true;
    await type(composer, "Accepted despite snapshot failure");
    await key("Enter");
    await until(() => contains("Accepted despite snapshot failure"), "accepted message is read back");
    await until(() => evaluate(`document.querySelector('[data-slot="chat-composer"]').getAttribute('aria-busy') !== 'true'`), "reconciliation finished");
    assert.equal(await draft(), "");
    assert.equal(await evaluate(`!!document.querySelector('[data-slot="chat-composer"] [role="alert"]')`), false);
    assert.equal(mutations("run/reply").length, before + 1);
  });

  await check("an accepted reply clears only its own draft after navigating to another chat", async () => {
    holdNextReply = true;
    await type(composer, "Accepted after switching chats");
    await key("Enter");
    await until(() => heldReply !== null, "reply acceptance is held");
    assert.equal(await evaluate(`document.querySelector('[data-slot="chat-composer-status"]').textContent`), "Sending message…");
    assert.equal(await draft(), "");
    assert.equal(await evaluate(`document.querySelector('[data-slot="chat-composer"]').getAttribute('aria-busy')`), 'true');
    await open(other, "Mock other answer");
    await type(composer, "Other chat draft stays");
    await heldReply.release();
    heldReply = null;
    await pause(100);
    assert.equal(await activeIs(other), true);
    assert.equal(await draft(), "Other chat draft stays");
    await open(main, "Mock main answer");
    assert.equal(await draft(), "", "accepted draft does not return after its composer unmounted");
    assert.equal(await evaluate(`document.querySelector('[data-slot="chat-composer-status"]').textContent.includes('Sending message')`), false);
  });

  await check("folded tool calls load only when opened and retain the original step coordinates", async () => {
    assert.equal(mutations("run/steps").length, 0);
    const folded = `${chat} [data-slot="chat-tool-call-trigger"]`;
    assert.equal(await evaluate(`${q(`${chat} [data-slot="chat-tool-call-title"]`)}.textContent`), "1 call · exec_command", "collapsed activity shows only a short tool name, without a shell command or preview");
    await click(folded);
    await until(() => mutations("run/steps").length === 1, "folded tool detail request");
    assert.deepEqual(mutations("run/steps")[0].body, { name: main.agent, run_id: main.runId, turn: 1, from: 0, to: 1 });
    const nestedTool = `${chat} [data-slot="chat-tool-call"] [data-slot="chat-tool-call"]`;
    await until(() => evaluate(`!!${q(nestedTool)}`), "expanded individual tool row");
    assert.equal(await evaluate(`${q(`${nestedTool} [data-slot="chat-tool-call-title"]`)}.textContent`), "exec_command", "individual tool rows also omit the shell invocation");
    await click(`${nestedTool} [data-slot="chat-tool-call-trigger"]`);
    await until(() => contains("git status --short"), "expanded tool arguments");
    assert.equal(await evaluate(`${q(`${nestedTool} dd`)}.textContent`), "/bin/zsh -lc 'git status --short'", "expanding the tool retains its complete literal command");
    await click(folded);
    await click(folded);
    await pause(80);
    assert.equal(mutations("run/steps").length, 1, "settled tool details are cached");
  });

  await check("assistant messages and tool activity leave a clear opposite-side gap on desktop and mobile", async () => {
    await open(compact, "Assistant layout fixture");
    for (const mobile of [false, true]) {
      const mode = mobile ? "mobile" : "desktop";
      await send("Emulation.setDeviceMetricsOverride", { width: mobile ? 390 : 1280, height: 1000, deviceScaleFactor: 1, mobile });
      await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
      const layout = await evaluate(`(() => {
        const content = ${q(`${chat} [data-slot="chat-thread-content"]`)};
        const style = getComputedStyle(content);
        const rect = content.getBoundingClientRect();
        const left = rect.left + parseFloat(style.paddingLeft);
        const right = rect.right - parseFloat(style.paddingRight);
        const measure = selector => {
          const node = content.querySelector(selector);
          const bounds = node.getBoundingClientRect();
          return { width: bounds.width, left: bounds.left, right: bounds.right };
        };
        return { left, right, available: right - left, user: measure('[data-from="user"]'), assistant: measure('[data-from="assistant"]'), tool: measure('[data-slot="chat-tool-call"]'), overflow: document.documentElement.scrollWidth > innerWidth };
      })()`);
      assert.ok(layout.available > 150, `${mode} transcript has usable width`);
      for (const kind of ["user", "assistant", "tool"]) {
        assert.ok(layout[kind].width <= layout.available * 0.9 + 1, `${mode} ${kind} is capped at 90% of the transcript content`);
      }
      assert.ok(Math.abs(layout.assistant.left - layout.left) <= 1, `${mode} assistant is left aligned`);
      assert.ok(Math.abs(layout.tool.left - layout.left) <= 1, `${mode} command activity is left aligned`);
      assert.ok(Math.abs(layout.user.right - layout.right) <= 1, `${mode} user is right aligned`);
      assert.ok(layout.right - layout.assistant.right >= layout.available * 0.1 - 1, `${mode} assistant leaves the right edge clear`);
      assert.ok(layout.user.left - layout.left >= layout.available * 0.1 - 1, `${mode} user leaves the left edge clear`);
      assert.equal(layout.overflow, false, `${mode} page does not overflow`);
      if (process.env.BROWSER_SCREENSHOTS_DIR) {
        await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
        const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
        await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, `design-system-app-compact-${mode}.png`), Buffer.from(screenshot.data, "base64"));
      }
    }
    await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
  });

  await check("grouped tool errors color only the error count while the failing tool stays red", async () => {
    const group = `${chat} [data-slot="chat-tool-call"]`;
    for (const dark of [false, true]) {
      await setTheme(dark);
      const colors = await evaluate(`(() => {
        const group = ${q(group)};
        const title = group.querySelector('[data-slot="chat-tool-call-title"]');
        const count = group.querySelector('[data-slot="chat-tool-call-error-count"]');
        const icon = group.querySelector('svg');
        const probe = document.createElement('span');
        probe.className = 'text-destructive';
        group.append(probe);
        const destructive = getComputedStyle(probe).color;
        probe.remove();
        return { title: title.textContent, titleColor: getComputedStyle(title).color, iconColor: getComputedStyle(icon).color, count: count?.textContent, countColor: count ? getComputedStyle(count).color : null, destructive };
      })()`);
      assert.equal(colors.title, "8 calls · exec_command, apply_patch", "collapsed groups list unique short tool names without previews or namespaces");
      assert.equal(colors.count, "1 error");
      assert.equal(colors.countColor, colors.destructive, "only the error count has the destructive color");
      assert.notEqual(colors.titleColor, colors.destructive, "group title remains neutral");
      assert.notEqual(colors.iconColor, colors.destructive, "group icon remains neutral");
    }
    await setTheme(false);
    const before = mutations("run/steps").length;
    await click(`${group} [data-slot="chat-tool-call-trigger"]`);
    await until(() => mutations("run/steps").length === before + 1, "compact group details request");
    await until(() => evaluate(`${q(group)}.querySelectorAll('[data-slot="chat-tool-call"]').length === 8`), "individual tool rows render");
    const nested = await evaluate(`(() => {
      const group = ${q(group)};
      const tools = [...group.querySelectorAll('[data-slot="chat-tool-call"]')];
      const failed = tools.find(node => node.dataset.status === 'error');
      const succeeded = tools.find(node => node.dataset.status === 'completed');
      const count = group.querySelector('[data-slot="chat-tool-call-error-count"]');
      return { title: group.querySelector('[data-slot="chat-tool-call-title"]').textContent, total: tools.length, toolNames: [...new Set(tools.map(tool => tool.querySelector('[data-slot="chat-tool-call-title"]').textContent))], failedColor: getComputedStyle(failed.querySelector('[data-slot="chat-tool-call-title"]')).color, failedIcon: getComputedStyle(failed.querySelector('svg')).color, normalColor: getComputedStyle(succeeded.querySelector('[data-slot="chat-tool-call-title"]')).color, errorColor: getComputedStyle(count).color, count: count.textContent };
    })()`);
    assert.equal(nested.total, 8);
    assert.equal(nested.title, "8 tool calls · exec_command, apply_patch");
    assert.deepEqual(nested.toolNames, ["exec_command", "apply_patch"], "all individual rows show only short tool names");
    assert.equal(nested.count, "1 error");
    assert.equal(nested.failedColor, nested.errorColor, "failing command remains red");
    assert.equal(nested.failedIcon, nested.errorColor, "failing command icon remains red");
    assert.notEqual(nested.normalColor, nested.errorColor, "successful command remains neutral");
    const failedTool = `${group} [data-slot="chat-tool-call"][data-status="error"]`;
    await click(`${failedTool} [data-slot="chat-tool-call-trigger"]`);
    await until(() => contains("git missing-fixture-command"), "failed tool arguments expand");
    assert.equal(await evaluate(`${q(`${failedTool} dd`)}.textContent`), "git missing-fixture-command", "failed command remains available as a literal argument");
    if (process.env.BROWSER_SCREENSHOTS_DIR) {
      await send("Emulation.setDeviceMetricsOverride", { width: 390, height: 1000, deviceScaleFactor: 1, mobile: true });
      await evaluate(`${q(`${group} [data-slot="chat-tool-call-trigger"]`)}.scrollIntoView({block:'start'})`);
      await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
      const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
      await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, "design-system-app-compact-errors-mobile.png"), Buffer.from(screenshot.data, "base64"));
      await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
    }
    await open(main, "Mock main answer");
  });

  await check("an opened group of calls keeps its opened rows while the group grows and settles", async () => {
    const settled = copy(snapshots.get(idOf(compact)));
    const snapshot = snapshots.get(idOf(compact));
    const call = (index, status = "ok") => ({
      kind: "tool", name: "mcp__adi__PyCode", input: JSON.stringify({ code: `print("growing-step-${index}")` }),
      status, output: status === "running" ? "" : `growing-output-${index}`,
    });
    const steps = [call(0), call(1, "running")];
    const header = () => ({
      kind: "calls", from: 0, to: steps.length, count: steps.length, tools: ["mcp__adi__PyCode"], preview: "",
      status: steps.some((step) => step.status === "running") ? "running" : "ok",
    });
    const answer = { ...assistant("", 3, [header()]), pending: true };
    snapshot.turns.push(user("Growing run request", 2), answer);
    snapshot.total_turns = snapshot.turns.length;
    snapshot.running = true;
    liveSteps.set(idOf(compact), { turn: answer.seq, steps });

    await open(compact, "Growing run request");
    const group = `${chat} [data-pending] [data-slot="chat-tool-call"]`;
    const rows = () => evaluate(`[...${q(group)}.querySelectorAll('[data-slot="chat-tool-call"]')].map(row => row.dataset.status + ':' + row.dataset.state)`);
    await until(() => evaluate(`!!${q(`${group} [data-slot="chat-tool-call-trigger"]`)}`), "running group renders");
    await click(`${group} [data-slot="chat-tool-call-trigger"]`);
    await until(async () => (await rows()).length === 2, "the group's two calls render");
    const first = `${group} [data-slot="chat-tool-call"]`;
    await click(`${first} [data-slot="chat-tool-call-trigger"]`);
    await until(() => contains("growing-output-0"), "the first call opens");
    // The mark lives on the DOM node, so it is gone if the row is ever built again.
    await evaluate(`(() => {
      const group = ${q(group)};
      const row = ${q(first)};
      row.dataset.probe = 'kept';
      row.querySelector('[data-slot="chat-tool-output"]').dataset.probe = 'kept-output';
      window.__groupReloads = 0;
      window.__groupObserver = new MutationObserver(() => { if (group.textContent.includes('Loading activity')) window.__groupReloads++; });
      window.__groupObserver.observe(group, { childList: true, subtree: true, characterData: true });
    })()`);
    const kept = () => evaluate(`(() => {
      const row = document.querySelector('[data-probe="kept"]');
      return { row: row?.isConnected === true, state: row?.dataset.state, output: document.querySelector('[data-probe="kept-output"]')?.isConnected === true, reloads: window.__groupReloads };
    })()`);

    // A call finishes and the next one starts: the header's range and count both move.
    steps[1] = call(1);
    steps.push(call(2, "running"));
    answer.steps = [header()];
    await until(async () => (await rows()).length === 3, "the new call joins the opened group", 9000);
    assert.deepEqual(await rows(), ["completed:open", "completed:closed", "running:closed"]);
    assert.deepEqual(await kept(), { row: true, state: "open", output: true, reloads: 0 }, "the opened call is the same row, still open, and the list was never replaced by a loading line");
    assert.equal(await evaluate(`${q(`${group} [data-slot="chat-tool-call-title"]`)}.textContent`), "3 tool calls · PyCode");

    // A refresh that fails says so under the calls it could not replace, and a retry catches up.
    failLiveSteps = true;
    steps[2] = call(2);
    steps.push(call(3, "running"));
    answer.steps = [header()];
    const alert = `${group} [data-slot="chat-tool-call-content"] [role="alert"]`;
    await until(() => mutations("run/steps").some((request) => request.body.run_id === compact.runId && request.body.to === 4), "the grown range is asked for", 9000);
    await until(() => evaluate(`${q(alert)}?.textContent.startsWith('Could not refresh activity details.') === true`), "the failed refresh is reported", 9000);
    assert.equal((await rows()).length, 3, "the calls already read stay on screen");
    assert.deepEqual(await kept(), { row: true, state: "open", output: true, reloads: 0 }, "a failed refresh keeps the opened call as it was");
    failLiveSteps = false;
    await click(`${alert} button`);
    await until(async () => (await rows()).length === 4, "the retry reads the calls that were missed", 9000);
    assert.equal(await evaluate(`!!${q(alert)}`), false, "the report clears once a read succeeds");
    assert.deepEqual(await kept(), { row: true, state: "open", output: true, reloads: 0 });

    // The answer ends: the header's status moves from running to done.
    steps[3] = call(3);
    answer.steps = [header()];
    answer.text = "Growing run finished";
    answer.pending = false;
    snapshot.running = false;
    await until(() => contains("Growing run finished"), "the answer settles", 9000);
    const done = `${chat} [data-slot="chat-message"]:last-of-type [data-slot="chat-tool-call"]`;
    await until(() => evaluate(`[...${q(done)}.querySelectorAll('[data-slot="chat-tool-call"]')].every(row => row.dataset.status === 'completed')`), "the last call settles", 9000);
    assert.deepEqual(await kept(), { row: true, state: "open", output: true, reloads: 0 }, "settling the group keeps the opened call as it was");
    assert.equal(await evaluate(`${q(done)}.querySelectorAll('[data-slot="chat-tool-call"]').length`), 4);

    await evaluate("window.__groupObserver.disconnect()");
    liveSteps.delete(idOf(compact));
    snapshots.set(idOf(compact), settled);
    await open(main, "Mock main answer");
  });

  await check("child-agent activity has a real conversation link and opens that chat", async () => {
    const link = `${chat} [data-kind="agent"] [data-slot="chat-tool-call-link"]`;
    assert.equal(await evaluate(`${q(link)}.getAttribute('href')`), `/interface?agent=${child.agent}&run=${child.runId}`);
    await click(link);
    await until(() => activeIs(child), "child conversation route");
    await until(() => contains("Mock child answer"), "child conversation transcript");
  });

  await check("new chat offers starred agents without inheriting the last viewed agent", async () => {
    await open(child, "Mock child answer");
    await click('[data-slot="app-chat-sidebar"] [aria-label="New conversation"]');
    await until(() => evaluate(`!!document.querySelector('[data-slot="new-chat-starred-agent"]')`), "starred agents shown");
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-app-chat-agent] option')).filter(o=>o.value).map(o=>o.value)`), ['design-assistant', 'layout-reviewer']);
    assert.equal(await evaluate(`document.querySelector('[data-slot="new-chat-starred-agent"]').textContent.includes('design-assistant')`), true);
    await click('[data-slot="new-chat-starred-agent"]');
    await until(() => evaluate(`new URLSearchParams(location.search).get('agent')==='design-assistant'`), "starred selection changes destination");
    assert.equal(await evaluate(`new URLSearchParams(location.search).has('run')`), false);
    await open(child, "Mock child answer");
  });

  await check("new chat starts the selected agent and opens the run returned by the server", async () => {
    await click('[data-slot="app-chat-sidebar"] [aria-label="New conversation"]');
    await until(() => evaluate(`new URLSearchParams(location.search).get('agent')==='design-assistant' && !new URLSearchParams(location.search).has('run')`), "new chat starts with a browser-local default");
    await evaluate(`history.pushState({},'', '/interface?agent=layout-reviewer'); dispatchEvent(new PopStateEvent('popstate'))`);
    await until(() => evaluate(`document.querySelector('[data-app-chat-agent]')?.value==='layout-reviewer'`), "explicit unstarred agent selected");
    // Router history updates before React necessarily remounts the conversation-keyed composer.
    await until(() => evaluate(`${q(`${chat} [data-slot="app-chat-agent"]`)} !== null && ${q(composer)}?.disabled === false && ${q(`${chat} [data-slot="chat-thread"]`)} === null`), "new-chat form has rendered before typing");
    await type(composer, "New fixture request");
    assert.equal(await draft(), "New fixture request");
    assert.equal(await evaluate(`${q(composer)}.disabled`), false, "selected agent can start a conversation");
    await key("Enter");
    await until(() => mutations("run").length === 1, "new-chat launch request");
    await until(() => evaluate(`new URLSearchParams(location.search).get('run')==='fixture-created-1'`), "created chat route opens");
    await until(() => contains("Mock new chat answer"), "created chat transcript");
    assert.equal(mutations("run").length, 1);
    assert.equal(mutations("run")[0].body.name, child.agent);
    assert.equal(mutations("run")[0].body.launched_by, "human");
    assert.equal(await draft(), "");
  });

  await check("Home New chat opens a blank conversation even when chat history exists", async () => {
    await open(main, "Mock main answer");
    const startsBefore = mutations("run").length;
    const repliesBefore = mutations("run/reply").length;
    await click('a[aria-label="ADI home"]');
    await until(() => evaluate(`location.pathname==='/' && ${q('button[data-slot="home-new-chat"][aria-label="New chat"]')} !== null`), "Home route and New chat button render");
    await click('button[data-slot="home-new-chat"][aria-label="New chat"]');
    await until(() => evaluate(`location.pathname==='/interface' && !new URLSearchParams(location.search).has('run') && ${q(`${chat} [data-slot="app-chat-agent"]`)} !== null && ${q(composer)}?.disabled === false && ${q(`${chat} [data-slot="chat-thread"]`)} === null`), "Home opens a new-chat form rather than the latest conversation");
    assert.equal(await draft(), "", "Home New chat starts with an empty composer");
    assert.equal(await evaluate(`new URLSearchParams(location.search).get('agent')`), child.agent, "last locally launched agent wins over the viewed conversation");
    assert.deepEqual(await evaluate(`JSON.parse(localStorage.getItem('adi.last-launched-agent.v1'))`), {agent: child.agent});
    assert.equal(await contains("Mock main answer"), false, "previous history is absent from the new chat");
    assert.equal(mutations("run").length, startsBefore, "opening a blank chat does not launch an agent");
    assert.equal(mutations("run/reply").length, repliesBefore, "opening a blank chat does not send a reply");
    await open(main, "Mock main answer");
  });

  await check("a late new-chat response does not pull the user away from the component gallery", async () => {
    await click('[data-slot="app-chat-sidebar"] [aria-label="New conversation"]');
    await until(() => evaluate(`${q(`${chat} [data-slot="app-chat-agent"]`)} !== null && ${q(composer)}?.disabled === false && ${q(`${chat} [data-slot="chat-thread"]`)} === null`), "new-chat form has rendered before typing");
    await type(composer, "Start then visit the gallery");
    assert.equal(await draft(), "Start then visit the gallery");
    assert.equal(await evaluate(`${q(`${chat} button[type="submit"]`)}.disabled`), false, "new chat has an enabled send action");
    holdNextStart = true;
    await key("Enter");
    await until(() => heldStart !== null, "new-chat response is held");
    await click('a[aria-label="ADI home"]');
    await until(() => evaluate(`location.pathname==='/' && ${q('[data-slot="home-app-link"][href="/library"]')} !== null`), "Home route and component app render before opening the gallery");
    await click('[data-slot="home-app-link"][href="/library"]');
    await until(() => evaluate("location.pathname==='/library'"), "component gallery route");
    await heldStart.release();
    heldStart = null;
    await pause(150);
    assert.equal(await evaluate("location.pathname"), "/library");
    await click('a[aria-label="Chats"]');
    await until(() => evaluate(`!!${q(listLink(main))}`), "chat sidebar returns");
    await open(main, "Mock main answer");
  });

  await check("running conversations accept queued replies and stop only the selected run", async () => {
    const snapshot = snapshots.get(idOf(main));
    snapshot.running = true;
    snapshot.turns.push({ ...assistant("Mock streaming answer", snapshot.turns.length), pending: true });
    snapshot.total_turns = snapshot.turns.length;
    await open(main, "Mock streaming answer");
    await until(() => evaluate(`!!${q(`${chat} [aria-label="Stop response"]`)}`), "running chat exposes Stop");
    const runningDot = await sidebarStatus(main, "Running");
    assert.ok(runningDot[3] > 0 && runningDot[2] > runningDot[0] && runningDot[2] > runningDot[1], "running chat has a visible blue dot in the selected theme");
    const before = mutations("run/reply").length;
    await type(composer, "Queued fixture follow-up");
    await key("Enter");
    await until(async () => mutations("run/reply").length === before + 1 && await draft() === "", "queued reply is accepted");
    assert.equal(mutations("run/reply").at(-1).body.mode, "regular");
    await until(() => evaluate(`${q(chat)}.querySelector('[data-queued="true"]') !== null`), "queued turn renders");
    assert.equal(await evaluate(`${q('[data-slot="chat-queued-message"]')}.open`), false);
    const queueCaption = `document.querySelector('[data-slot="chat-queue"] [data-slot="chat-queue-caption"]')`;
    assert.equal(await evaluate(`${q('[data-slot="chat-queue"]')}.getAttribute('aria-label')`), "Queued messages");
    assert.equal(await evaluate(queueCaption), null, "one queued message is not a batch");
    if (process.env.BROWSER_SCREENSHOTS_DIR) {
      await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
      const capture = await send("Page.captureScreenshot", {format:"png"});
      await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR,"queued-message.png"),Buffer.from(capture.data,"base64"));
    }
    assert.equal(await evaluate(`${q('[data-slot="chat-thread"]')}?.querySelector('[data-queued="true"]') ?? null`), null, "queued message is outside the transcript");
    await click('[data-slot="chat-queued-message"] summary');
    assert.equal(await evaluate(`${q('[data-slot="chat-queued-message"]')}.open`), true);
    assert.equal(await evaluate(`!!${q(chat)}.querySelector('[aria-label="Send ASAP"]')`), false, "composer only offers regular queue submission");
    assert.equal(mutations("run/reply").length, before + 1);
    await type(composer, "Second queued follow-up");
    await key("Enter");
    const queuedRows = `document.querySelectorAll('[data-slot="chat-queued-row"]')`;
    await until(() => evaluate(`${queuedRows}.length === 2`), "second queued turn renders");
    assert.equal(await evaluate(`${queueCaption}?.textContent`), "2 messages will be sent together", "two queued messages say they go as one batch");
    snapshot.caps = { ...capabilities, batches: false };
    await until(() => evaluate(`${queuedRows}.length === 2 && ${queueCaption} === null`), "a server that asks one message per turn is not said to batch");
    snapshot.caps = capabilities;
    await until(() => evaluate(`${queueCaption} !== null`), "the caption follows the server's capability");
    if (process.env.BROWSER_SCREENSHOTS_DIR) {
      const capture = await send("Page.captureScreenshot", {format:"png"});
      await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR,"queued-batch.png"),Buffer.from(capture.data,"base64"));
    }
    const cancelQueued = '[data-slot="chat-queued-cancel"]';
    assert.equal(await evaluate(`${q(cancelQueued)}.closest('summary')`), null, "Cancel is not nested in the disclosure control");
    assert.equal(await evaluate(`${q(cancelQueued)}.getAttribute('aria-label')`), "Cancel queued message: Queued fixture follow-up");
    await click(cancelQueued);
    await until(() => mutations("run/unqueue").length === 1, "queued message cancel request");
    assert.deepEqual(mutations("run/unqueue")[0].body, { name: main.agent, run_id: main.runId, index: 0, limit: 20, fold: true });
    await until(() => evaluate(`${queuedRows}.length === 1 && ${queuedRows}[0].textContent.includes('Second queued follow-up')`), "only the cancelled message leaves the queue");
    assert.equal(await evaluate(queueCaption), null, "the caption goes when one message is left");
    assert.equal(await evaluate(`${q('[data-slot="chat-thread"]')}.textContent.includes('Queued fixture follow-up')`), false, "a cancelled message never reaches the transcript");
    await click(cancelQueued);
    await until(() => evaluate(`${queuedRows}.length === 0`), "the last queued message can be cancelled too");
    assert.equal(await evaluate(`document.querySelector('[data-slot="chat-queue"]')`), null, "an empty queue renders no group");
    assert.equal(mutations("run/unqueue").length, 2);
    assert.equal(mutations("run/reply").length, before + 2, "cancelling never resends a message");
    await click(`${chat} [aria-label="Stop response"]`);
    await until(() => mutations("run/stop").length === 1, "Stop request");
    assert.deepEqual(mutations("run/stop")[0].body, { name: main.agent, run_id: main.runId });
    await until(() => evaluate(`!${q(`${chat} [aria-label="Stop response"]`)}`), "stopped chat removes Stop");
  });

  await check("await status explains unfinished chats and clears as tasks resolve", async () => {
    const snapshot = snapshots.get(idOf(main));
    snapshot.running = false;
    snapshot.awaits = [
      { id: "build", note: "Waiting for the build to finish.", summary: "on build.finished", check: "private-build-command --token secret" },
      { id: "review", note: "", summary: "Waiting for the layout reviewer’s result.", check: "private-review-command" },
    ];
    await open(main, "Mock main answer");
    await click('[aria-label="Refresh chat"]');
    const status = '[data-slot="chat-waiting-status"]';
    await until(() => evaluate(`${q(status)}?.textContent.includes('Waiting on 2 tasks')`), "two pending tasks shown");
    assert.equal(await evaluate(`${q(status)}.open`), false, "waiting status starts compact");
    assert.ok(await evaluate(`${q(status)}.getBoundingClientRect().height<=36`), "collapsed waiting status fits one line");
    assert.equal(await evaluate(`${q(status)}.querySelector('ul').checkVisibility()`), false);
    await click(`${status} summary`);
    assert.equal(await evaluate(`${q(status)}.open`), true);
    assert.ok(await evaluate(`${q(status)}.querySelector('ul').checkVisibility()`));
    await key("Enter");
    assert.equal(await evaluate(`${q(status)}.open`), false, "keyboard collapses details");
    assert.ok(await evaluate(`${q(status)}.textContent.includes('Waiting for the build to finish.')`));
    assert.ok(await evaluate(`${q(status)}.textContent.includes('Waiting for the layout reviewer’s result.')`));
    assert.equal(await evaluate(`${q(status)}.textContent.includes('private-')`), false);
    assert.equal(await evaluate(`${q('[data-slot="chat-thread"]')}.contains(${q(status)})`), false, "status stays outside scrolling history");
    await open(other, "Mock other answer");
    assert.equal(await evaluate(`!!${q(status)}`), false, "other chat has no stale waits");
    await open(main, "Mock main answer");
    await until(() => evaluate(`!!${q(status)}`), "waits restore with their chat");
    if (process.env.BROWSER_SCREENSHOTS_DIR) {
      await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
      const capture = await send("Page.captureScreenshot", {format:"png"});
      await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR,"waiting-status.png"),Buffer.from(capture.data,"base64"));
    }
    const cancelWait = `${status} [data-slot="chat-waiting-cancel"]`;
    assert.equal(await evaluate(`${q(cancelWait)}.checkVisibility()`), false, "Cancel stays out of the collapsed status");
    await click(`${status} summary`);
    assert.equal(await evaluate(`document.querySelectorAll(${JSON.stringify(cancelWait)}).length`), 2);
    assert.equal(await evaluate(`${q(cancelWait)}.getAttribute('aria-label')`), "Cancel waiting: Waiting for the build to finish.");
    await click(cancelWait);
    await until(() => mutations("await/ignore").length === 1, "await cancel request");
    assert.deepEqual(mutations("await/ignore")[0].body, { name: main.agent, run_id: main.runId, id: "build" });
    await until(() => evaluate(`${q(status)}?.textContent.includes('Waiting on 1 task') && !${q(status)}.textContent.includes('build to finish')`), "cancelled wait leaves the status");
    assert.equal(await evaluate(`${q(status)}.open`), true, "remaining waits stay expanded");
    fireAwaitOnCancel = true;
    await click(cancelWait);
    await until(() => evaluate(`${q(`${chat} [role="alert"]`)}?.textContent.includes('no longer waiting')`), "a wait that fired first is reported, not cancelled");
    assert.deepEqual(mutations("await/ignore")[1].body, { name: main.agent, run_id: main.runId, id: "review" });
    await until(() => evaluate(`!${q(status)}`), "a fired wait leaves the status");
    snapshot.awaits = [{ id: "unknown", note: " ", summary: "", check: "never-show-this-command" }];
    await click('[aria-label="Refresh chat"]');
    await until(() => evaluate(`${q(status)}?.textContent.includes('Waiting on 1 task')`), "count updates");
    assert.equal(await evaluate(`!!${q(`${chat} [role="alert"]`)}`), false, "a refresh clears the reported cancel failure");
    assert.ok(await evaluate(`${q(status)}.textContent.includes('Waiting for a background task to finish.')`));
    snapshot.awaits = [];
    await click('[aria-label="Refresh chat"]');
    await until(() => evaluate(`!${q(status)}`), "resolved tasks remove status");
  });

  await check("pending questions submit the current ask id and ordered replies", async () => {
    await open(question, "Mock question context");
    await until(() => evaluate(`!!document.querySelector('[data-slot="app-chat-question"]')`), "pending question form");
    const questionDot = await sidebarStatus(question, "Needs your answer");
    assert.ok(questionDot[3] > 0 && questionDot[0] > questionDot[1] && questionDot[1] > questionDot[2], "question chat has a visible amber dot in the selected theme");
    await type('[data-slot="app-chat-answer-input"]', "Use the compact label");
    await click('[data-slot="app-chat-answer-submit"]');
    await until(() => mutations("run/answer").length === 1, "question answer request");
    const answer = mutations("run/answer")[0].body;
    assert.equal(answer.name, question.agent);
    assert.equal(answer.run_id, question.runId);
    assert.equal(answer.ask, "fixture-ask-17");
    assert.deepEqual(answer.replies, ["Use the compact label"]);
    await until(() => evaluate(`!document.querySelector('[data-slot="app-chat-question"]')`), "settled question form disappears");
  });

  await check("questions accept partial answers and choices with additional text", async () => {
    await open(other, "Mock other answer");
    snapshots.get(idOf(question)).pending_question = {
      id: "fixture-partial", headline: "Optional questions", questions: [
        { question: "Leave this unanswered?", options: [], multi_select: false },
        { question: "Choose a label?", options: [{ label: "Compact" }], multi_select: false },
        { question: "Anything else?", options: [], multi_select: false },
      ],
    };
    await open(question, "Mock question context");
    const submit = '[data-slot="app-chat-answer-submit"]';
    const input = '[data-slot="app-chat-question"] fieldset:nth-of-type(2) textarea';
    await until(() => evaluate(`document.querySelectorAll('[data-slot="app-chat-answer-input"]').length === 3`), "multiple questions");
    assert.equal(await evaluate(`document.querySelector('${submit}').disabled`), true);
    await type(input, "   ");
    assert.equal(await evaluate(`document.querySelector('${submit}').disabled`), true);
    await click('[data-slot="app-chat-question"] input[type="radio"]');
    await click('[data-slot="app-chat-question"] input[type="radio"]');
    assert.equal(await evaluate(`document.querySelector('${submit}').disabled`), true, "a choice can be cleared");
    await click('[data-slot="app-chat-question"] input[type="radio"]');
    await type(input, "Keep it short");
    await click(submit);
    await until(() => mutations("run/answer").length === 2, "partial answer request");
    assert.deepEqual(mutations("run/answer")[1].body.replies, ["", "Compact, Keep it short", ""]);
    await until(() => evaluate(`!document.querySelector('[data-slot="app-chat-question"]')`), "partial answer settles");
  });

  await check("expand workspace keeps the inner sidebar and restores the outer navigation", async () => {
    await open(main, "Mock main answer");
    await type(composer, "Draft while expanded");
    const width = await evaluate(`document.querySelector('[data-slot="app-secondary-navigation"]').getBoundingClientRect().width`);
    await click('[aria-label="Expand workspace"]');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('[data-slot="app-sidebar"]')).display`), 'none');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('[data-slot="app-topbar"]')).display`), 'none');
    assert.equal(await evaluate(`document.querySelector('[data-slot="app-secondary-navigation"]').hidden`), false);
    assert.deepEqual(await evaluate(`(() => {const r=document.querySelector('[data-slot="app-layout-surface"]').getBoundingClientRect();return [r.x,r.y,r.width,r.height]})()`), await evaluate(`[0,0,document.body.clientWidth,innerHeight]`));
    assert.equal(await evaluate(`document.querySelector('[data-slot="app-secondary-navigation"]').getBoundingClientRect().width`), width);
    assert.equal(await draft(), 'Draft while expanded');
    await send('Emulation.setDeviceMetricsOverride', {width:390,height:844,deviceScaleFactor:1,mobile:false});
    assert.deepEqual(await evaluate(`(() => {const r=document.querySelector('[data-slot="app-layout-surface"]').getBoundingClientRect();return [r.x,r.y,r.width,r.height]})()`), await evaluate(`[0,0,document.body.clientWidth,innerHeight]`));
    await send('Emulation.setDeviceMetricsOverride', {width:1280,height:900,deviceScaleFactor:1,mobile:false});
    await click('[aria-label="Restore navigation"]');
    assert.equal(await evaluate(`document.querySelector('[data-slot="app-secondary-navigation"]').getBoundingClientRect().width`), width);
    assert.equal(await draft(), 'Draft while expanded');
    assert.equal(await evaluate(`document.activeElement.getAttribute('data-slot')`), 'workspace-expand');
    await type(composer, '');
  });

  await check("sidebar context menu renames and stars an inactive chat", async () => {
    await open(main, "Mock main answer");
    const menu = '[data-slot="chat-context-menu"]';
    const rightClick = async () => evaluate(`${q(listLink(other))}.dispatchEvent(new MouseEvent('contextmenu', {bubbles:true,cancelable:true,clientX:220,clientY:300}))`);
    await rightClick();
    await until(() => evaluate(`!!document.querySelector('${menu}')`), "context menu opens");
    holdNextEdit = true;
    await click(`${menu} button:nth-of-type(2)`);
    await until(() => heldEdit !== null, "star request held");
    await until(() => evaluate(`${q(listLink(other))}?.querySelector('[aria-label="Starred chat"]') !== null`), "star updates before response");
    const indexReadsBeforeStar = requests.filter((request) => request.method === "GET" && request.path === "/api/app/agents/runs/all").length;
    heldEdit();
    await until(() => evaluate(`!document.querySelector('${menu}')`), "star saved");
    assert.ok(stars.has(`${other.agent}:${other.runId}`));
    assert.ok(await evaluate(`${q(listLink(main))}.getAttribute('aria-current')==='page'`));
    // The star edit is still pending until its follow-up index read lands; a second edit of the
    // same field in that window is deliberately dropped, so wait for the read before un-starring.
    await until(() => requests.filter((request) => request.method === "GET" && request.path === "/api/app/agents/runs/all").length > indexReadsBeforeStar, "star refresh read");
    await pause(100);
    await rightClick();
    assert.equal(await evaluate(`document.querySelector('${menu} button:nth-of-type(2)').textContent`), "Unstar");
    await click(`${menu} button:nth-of-type(2)`);
    await until(() => mutations("run/star").length === 2, "unstar request sent");
    await until(() => evaluate(`!document.querySelector('${menu}')`), "unstar saved");
    await rightClick();
    await click(`${menu} button`);
    await type('#context-chat-name', 'Context renamed');
    failNextRename = true;
    await key('Enter');
    await until(() => evaluate(`document.querySelector('[data-slot="chat-mutation-error"]')?.textContent.includes('Fixture rename refused')`), "context rename failure notification");
    assert.equal(await evaluate(`!!document.querySelector('${menu}')`), false);
    await rightClick();
    await click(`${menu} button`);
    await type('#context-chat-name', 'Context renamed');
    holdIndexRefresh = true;
    await key('Enter');
    await until(() => heldIndexes.length > 0, "rename refresh held");
    await until(() => evaluate(`!document.querySelector('${menu}')`), "context rename closes before refresh");
    await releaseIndexRefresh();
    await until(() => evaluate(`${q(listLink(other))}?.textContent.includes('Context renamed')`), "renamed sidebar refreshed");
    assert.ok(await evaluate(`${q(listLink(other))}.textContent.includes('Context renamed')`));
    await evaluate(`${q(listLink(other))}.focus(); ${q(listLink(other))}.dispatchEvent(new KeyboardEvent('keydown',{key:'F10',shiftKey:true,bubbles:true,cancelable:true}))`);
    await until(() => evaluate(`!!document.querySelector('${menu}')`), "keyboard context menu opens");
    await key('Escape');
    assert.equal(await evaluate(`document.activeElement===${q(listLink(other))}`), true);
  });

  await check("Hide persists, removes an inactive chat and preserves the current chat", async () => {
    const menu = '[data-slot="chat-context-menu"]';
    await evaluate(`${q(listLink(other))}.dispatchEvent(new MouseEvent('contextmenu', {bubbles:true,cancelable:true,clientX:220,clientY:300}))`);
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('${menu} [role="menuitem"]'), item => item.textContent)`), ['Rename', 'Star', 'Hide']);
    failNextHide = true;
    holdNextEdit = true;
    await click(`${menu} button:nth-of-type(3)`);
    await until(() => heldEdit !== null, "hide request held");
    await until(() => evaluate(`!${q(listLink(other))}`), "row removed before response");
    heldEdit();
    await until(() => evaluate(`document.querySelector('[data-slot="chat-mutation-error"]')?.textContent.includes('Fixture hide refused')`), "hide error is visible");
    assert.ok(await evaluate(`!!${q(listLink(other))}`));
    holdIndexRefresh = true;
    await evaluate(`${q(listLink(other))}.dispatchEvent(new MouseEvent('contextmenu', {bubbles:true,cancelable:true,clientX:220,clientY:300}))`);
    await click(`${menu} button:nth-of-type(3)`);
    await until(() => heldIndexes.length > 0, "hide refresh held");
    await until(() => evaluate(`!document.querySelector('${menu}')`), "hide closes before refresh");
    await releaseIndexRefresh();
    await until(() => evaluate(`!${q(listLink(other))}`), "hidden chat removed");
    assert.ok(hiddenRuns.has(`${other.agent}:${other.runId}`));
    assert.ok(await evaluate(`${q(listLink(main))}.getAttribute('aria-current')==='page'`));
    await evaluate(`${q(listLink(main))}.dispatchEvent(new MouseEvent('contextmenu', {bubbles:true,cancelable:true,clientX:220,clientY:300}))`);
    await click(`${menu} button:nth-of-type(3)`);
    await until(() => evaluate(`!document.querySelector('${menu}') && !${q(listLink(main))} && !new URL(location.href).searchParams.has('run')`), "hiding the active chat opens a blank chat");
    hiddenRuns.clear();
    await until(() => evaluate(`!!${q(listLink(other))} && !!${q(listLink(main))}`), "fixture chats restored", 12000);
    await open(other, "Mock other answer");
    await open(main, "Mock main answer");
  });

  await check("a middle click on a row hides that chat and opens no tab", async () => {
    const pages = async () => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).filter((tab) => tab.type === "page").length;
    const tabsBefore = await pages();
    const hidesBefore = mutations("run/hide").length;
    await evaluate(`${q(listLink(other))}.scrollIntoView({block:'center'})`);
    const point = await evaluate(`(() => { const r=${q(listLink(other))}.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
    await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "middle", buttons: 4, clickCount: 1, ...point });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "middle", clickCount: 1, ...point });
    await until(() => evaluate(`!${q(listLink(other))}`), "middle-clicked chat leaves the list");
    await until(() => hiddenRuns.has(idOf(other)), "the hide reached the machine");
    assert.equal(mutations("run/hide").length, hidesBefore + 1, "one hide was sent");
    assert.ok(await activeIs(main), "the open chat stays open");
    assert.ok(await evaluate(`${q(listLink(main))}.getAttribute('aria-current')==='page'`));
    await pause(300);
    assert.equal(await pages(), tabsBefore, "the row did not open in a new tab");
    hiddenRuns.clear();
    await until(() => evaluate(`!!${q(listLink(other))}`), "fixture chat restored", 12000);
  });

  await check("failed stars roll back and report the error outside the menu", async () => {
    await open(main, "Mock main answer");
    failNextStar = true;
    holdNextEdit = true;
    await click('[aria-label="Star chat"]');
    await until(() => heldEdit !== null, "failed star request held");
    await until(() => evaluate(`!!document.querySelector('[aria-label="Unstar chat"]')`), "optimistic header star");
    heldEdit();
    await until(() => evaluate(`document.querySelector('[data-slot="chat-mutation-error"]')?.textContent.includes('Fixture star refused')`), "star failure notification");
    await until(() => evaluate(`!!document.querySelector('[aria-label="Star chat"]') && !${q(listLink(main))}?.querySelector('[aria-label="Starred chat"]')`), "failed star rolled back");
    await click('[aria-label="Dismiss chat error"]');
    assert.equal(await evaluate(`!!document.querySelector('[data-slot="chat-mutation-error"]')`), false);
  });

  await check("chat stars persist across navigation and can be removed", async () => {
    await open(main, "Mock main answer");
    await click('[aria-label="Star chat"]');
    await until(() => evaluate(`document.querySelector('[aria-label="Unstar chat"]')?.getAttribute('aria-pressed')==='true'`), "chat starred");
    assert.ok(await evaluate(`${q(listLink(main))}.querySelector('[aria-label="Starred chat"]') !== null`));
    await open(other, "Mock other answer");
    await open(main, "Mock main answer");
    await click('[aria-label="Unstar chat"]');
    await until(() => evaluate(`!!document.querySelector('[aria-label="Star chat"]:not(:disabled)')`), "chat unstarred");
    assert.equal(stars.size, 0);
  });

  await check("chat titles rename optimistically, report failures, cancel and restore the default", async () => {
    await open(main, "Mock main answer");
    await click('[aria-label="Rename chat"]');
    await type('[aria-label="Chat name"]', "My design review");
    failNextRename = true;
    await key("Enter");
    await until(() => evaluate(`document.querySelector('[data-slot="chat-mutation-error"]')?.textContent.includes('Fixture rename refused')`), "header rename failure notification");
    assert.equal(await evaluate(`!!document.querySelector('[data-slot="chat-rename"]')`), false);
    await click('[aria-label="Rename chat"]');
    await type('[aria-label="Chat name"]', "My design review");
    holdIndexRefresh = true;
    holdNextEdit = true;
    await click('[aria-label="Save chat name"]');
    await until(() => heldEdit !== null, "rename request held");
    await until(() => evaluate(`${q(chat)}.querySelector('h1')?.textContent==='My design review' && ${q(listLink(main))}?.textContent.includes('My design review')`), "rename updates before response");
    heldEdit();
    await until(() => heldIndexes.length > 0, "header rename refresh held");
    await until(() => evaluate(`!document.querySelector('[data-slot="chat-rename"]')`), "header rename closes before refresh");
    await releaseIndexRefresh();
    await until(() => evaluate(`${q(chat)}.querySelector('h1')?.textContent==='My design review'`), "header renamed");
    assert.ok(await evaluate(`${q(listLink(main))}.textContent.includes('My design review')`));
    await click('[aria-label="Rename chat"]');
    await type('[aria-label="Chat name"]', "Discard this name");
    await key("Escape");
    assert.equal(await evaluate(`${q(chat)}.querySelector('h1').textContent`), "My design review");
    await click('[aria-label="Rename chat"]');
    await type('[aria-label="Chat name"]', "");
    await key("Enter");
    await until(() => evaluate(`${q(chat)}.querySelector('h1')?.textContent==='Default fixture title'`), "default title restored");
  });

  await check("microphone dictation appends an unsent draft and releases audio on navigation", async () => {
    await open(main, "Mock main answer");
    assert.equal(await evaluate(`document.querySelector('[data-slot="chat-microphone"] select')`), null, "microphone has no engine picker");
    await evaluate(`void Object.defineProperty(window,'isSecureContext',{value:false,configurable:true})`);
    await open(other, "Mock other answer");
    assert.equal(await evaluate(`document.querySelector('[aria-label="Dictate message"]').disabled`), true, "HTTP microphone is blocked");
    assert.equal(await evaluate(`document.querySelector('[data-slot="chat-microphone"] a')`), null, "blocked microphone has no HTTPS link");
    assert.equal(await evaluate(`document.querySelector('[data-slot="chat-microphone"]').innerText.trim()`), "?", "blocked microphone only shows its help icon as text");
    assert.equal(await evaluate(`!!document.querySelector('[data-slot="chat-microphone"] [data-slot="help-tooltip-trigger"]')`), true, "blocked microphone retains its help trigger");
    if (process.env.BROWSER_SCREENSHOTS_DIR) {
      await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
      const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
      await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, "design-system-microphone-blocked.png"), Buffer.from(screenshot.data, "base64"));
    }
    await evaluate(`void Object.defineProperty(window,'isSecureContext',{value:true,configurable:true})`);
    await open(main, "Mock main answer");
    await type(composer,"Typed draft");
    const repliesBefore=mutations('run/reply').length;
    await until(() => evaluate(`!document.querySelector('[aria-label="Dictate message"]').disabled`), "microphone ready");
    await click('[aria-label="Dictate message"]');
    await until(() => evaluate(`!!window.__recognition`), "browser recognizer started");
    await evaluate(`window.__recognition.onresult({resultIndex:0,results:[{isFinal:true,0:{transcript:'Spoken fixture words'}}]})`);
    await until(async()=>await draft()==='Typed draft Spoken fixture words',"dictated words append");
    await click('[aria-label="Stop dictation"]');
    assert.equal(mutations('run/reply').length,repliesBefore,"dictation never sends");
    await evaluate(`localStorage.setItem('adi.voice.engine.v1','openai')`);
    await open(other, "Mock other answer");
    await open(main, "Mock main answer");
    await click('[aria-label="Dictate message"]');
    await until(() => evaluate(`!!document.querySelector('[aria-label="Stop dictation"]')`),"recording started");
    await click('[aria-label="Stop dictation"]');
    await until(async()=>await draft()==='Typed draft Spoken fixture words Recorded fixture words',"server transcription appends");
    assert.ok(await evaluate('window.__tracksStopped>0'));
    failTranscription=true;
    await click('[aria-label="Dictate message"]');
    await until(() => evaluate(`!!document.querySelector('[aria-label="Stop dictation"]')`),"second recording started");
    await click('[aria-label="Stop dictation"]');
    await until(()=>evaluate(`document.querySelector('[data-slot="chat-microphone"]').textContent.includes('Fixture transcription failed')`),"transcription failure reported");
    assert.equal(await draft(),'Typed draft Spoken fixture words Recorded fixture words');
    const stopped=await evaluate('window.__tracksStopped');
    await click('[aria-label="Dictate message"]');
    await until(()=>evaluate(`!!document.querySelector('[aria-label="Stop dictation"]')`),"recording before navigation");
    await open(other,"Mock other answer");
    assert.ok(await evaluate(`window.__tracksStopped>${stopped}`),"navigation releases microphone");
    assert.equal(mutations('run/reply').length,repliesBefore);
    await open(main,"Mock main answer");
    assert.equal(await draft(),'Typed draft Spoken fixture words Recorded fixture words');
    await type(composer,"");
  });

  await check("a return visit opens the chat page with what it last showed and catches up in the background", async () => {
    await open(main, "Mock main answer");
    await click('[aria-label="Refresh chat"]');
    const rows = await evaluate(`document.querySelectorAll('[data-slot="app-conversation-link"]').length`);
    const title = titles.get(idOf(main));
    await until(() => evaluate(`['fleet','agents','index','thread','voice'].every(kind => Object.keys(localStorage).some(key => key.startsWith('adi.chat.' + kind + '.v1')))`), "every read the page opens with has a saved copy");
    assert.equal(await evaluate(`Object.keys(localStorage).filter(key => key.startsWith('adi.chat.')).some(key => /tool_steps|history/.test(localStorage.getItem(key)) && !key.startsWith('adi.chat.thread.') && !key.startsWith('adi.chat.agents.'))`), false, "the saved list keeps only the capabilities it reads");
    const release = holdReads();
    const before = requests.length;
    await evaluate("window.__beforeReturnVisit = true");
    await send("Page.reload");
    await until(async () => await evaluate("!window.__beforeReturnVisit") && await contains("Mock main answer"), "the conversation is on screen while its reads are still out");
    await until(() => requests.slice(before).filter((request) => openingRead(request.method, request.path)).length >= 4, "the page is reading everything again");
    await pause(200);
    const held = await evaluate("window.chatFrames");
    assert.ok(held.length > 0);
    for (const frame of held) {
      assert.equal(frame.loading, "", `nothing says Loading: ${JSON.stringify(held)}`);
      if (frame.rows !== null) assert.equal(frame.rows, rows, `the list has its rows from the first frame: ${JSON.stringify(held)}`);
      if (frame.title !== null) assert.deepEqual([frame.title, frame.answers, frame.newChat, frame.composerOff], [title, "Mock main answer", false, false], `the conversation is whole from the first frame: ${JSON.stringify(held)}`);
    }
    assert.equal(await evaluate(`${q(listLink(main))}.getAttribute('aria-current')`), "page");
    // The machine's answer wins as soon as it arrives: a new reply and a new title.
    const snapshot = snapshots.get(idOf(main));
    snapshot.turns.push(assistant("Mock caught-up answer", snapshot.turns.length));
    snapshot.total_turns = snapshot.turns.length;
    titles.set(idOf(main), `${title}, continued`);
    release();
    await until(async () => await contains("Mock caught-up answer") && await evaluate(`${q(`${chat} header h1`)}.textContent === ${JSON.stringify(`${title}, continued`)}`), "the page catches up");
    const frames = await evaluate("window.chatFrames");
    assert.ok(frames.every((frame) => frame.loading === "" && (frame.rows === null || frame.rows === rows) && frame.newChat === false && frame.composerOff !== true), `catching up never empties or disables anything: ${JSON.stringify(frames)}`);
    snapshot.turns.pop();
    snapshot.total_turns = snapshot.turns.length;
    titles.set(idOf(main), title);
    await click('[aria-label="Refresh chat"]');
    await until(async () => !(await contains("Mock caught-up answer")) && await evaluate(`${q(`${chat} header h1`)}.textContent === ${JSON.stringify(title)}`), "the fixture is back as it was");
  });

  await check("an unaddressed chat page opens the saved newest conversation at once and follows the machine's newest", async () => {
    const visit = async () => {
      await evaluate("window.__beforeBareVisit = true");
      await send("Page.navigate", { url: `${origin}/interface` });
      await until(() => evaluate("!window.__beforeBareVisit && !!document.querySelector('[data-slot=\"app-chat\"]')"), "the chat page is mounted");
    };
    const settled = async (since) => {
      await until(() => requests.slice(since).some((request) => request.path === "/api/app/agents/runs/all"), "the list was read again");
      await pause(400);
    };
    // The saved list says the main conversation is the newest.
    let release = holdReads();
    await visit();
    await until(async () => await activeIs(main) && await contains("Mock main answer"), "the saved newest conversation opens with its saved page");
    const held = await evaluate("window.chatFrames");
    assert.ok(held.every((frame) => frame.loading === "" && frame.newChat === false && frame.title !== "New chat"), `no loading state and no new-chat screen on the way: ${JSON.stringify(held)}`);
    // Meanwhile another conversation became the newest: the machine's answer moves the page there.
    activity.set(idOf(other), 9000);
    release();
    await until(async () => await activeIs(other) && await contains("Mock other answer"), "the page follows the machine's newest");
    assert.equal(await evaluate("history.length") > 0, true);

    // Saved newest is now the other conversation, the machine's is the main one again. A reader
    // who has started writing is not moved.
    activity.clear();
    release = holdReads();
    let since = requests.length;
    await visit();
    await until(async () => await activeIs(other) && await contains("Mock other answer"), "the saved newest opens");
    await type(composer, "A draft begun before the list arrived");
    release();
    await settled(since);
    assert.equal(await activeIs(other), true, "a reader who is writing stays put");
    assert.equal(await draft(), "A draft begun before the list arrived");
    await type(composer, "");

    // Nor is a reader who has already gone to a conversation of their own choosing.
    await until(() => evaluate(`document.querySelector('[data-slot="app-conversation-link"]')?.dataset.run === ${JSON.stringify(main.runId)}`), "the list is the machine's again, the main conversation first");
    activity.set(idOf(other), 9000);
    release = holdReads();
    since = requests.length;
    await visit();
    await until(async () => await activeIs(main) && await contains("Mock main answer"), "the saved newest opens");
    await click(listLink(question));
    await until(() => activeIs(question), "the reader opens another conversation");
    release();
    await settled(since);
    assert.equal(await activeIs(question), true, "a reader who chose a conversation stays on it");
    activity.clear();
    await open(main, "Mock main answer");
    await click('[aria-label="Refresh chat"]');
    // Rows that are still moving back would put the next scenario's click on the wrong one.
    await until(() => evaluate(`[...document.querySelectorAll('[data-slot="app-conversation-link"]')].findIndex(link => link.dataset.run === ${JSON.stringify(other.runId)}) > 1`), "the list is back in the machine's order");
  });

  await check("a failed background poll goes to the event log, not into the page", async () => {
    await open(main, "Mock main answer");
    const button = '[data-slot="event-log-button"]';
    const panel = '[data-slot="event-log"]';
    const item = `${panel} [data-slot="event-log-item"]`;
    const rows = () => evaluate(`document.querySelectorAll('${item}').length`);
    const closed = (description) => until(() => evaluate(`document.querySelector('${panel}') === null`), description);
    // Earlier scenarios refused edits on purpose and those are mirrored into the log. The log
    // cannot be emptied, so opening it once marks them read and gives the count to start from.
    await click(button);
    await until(() => evaluate(`!!document.querySelector('${panel}')`), "the panel opens");
    assert.ok(await evaluate(`${q(panel)}.textContent.includes('Fixture hide refused')`), "a refused edit was recorded alongside its toast");
    const before = await rows();
    await key("Escape");
    await closed("the panel closes");
    assert.equal(await evaluate(`${q(button)}.dataset.unread`), undefined, "a log that was just read has nothing unread");
    failNextIndex = true;
    await until(() => evaluate(`${q(button)}.dataset.unread === '1'`), "the rail button shows one unread event", 9000);
    assert.equal(await evaluate(`!!document.querySelector('[data-slot="app-chat-sidebar"] [role="alert"]')`), false, "the sidebar shows no banner");
    assert.equal(await evaluate(`!!document.querySelector('${chat} > [role="alert"]')`), false, "the chat header shows no banner");
    assert.ok(await contains("Mock main answer"), "the transcript is untouched");
    assert.ok(await evaluate(`document.querySelectorAll('[data-slot="app-conversation-link"]').length > 0`), "the list keeps its rows");
    await click(button);
    await until(() => evaluate(`${q(item)}?.textContent.includes('Fixture index refused')`), "the panel lists the failure first");
    assert.equal(await evaluate(`${q(item)}.dataset.level`), "error");
    assert.ok(await evaluate(`${q(item)}.textContent.includes('This machine · Browser · Chats')`), "the row names the machine, where it was noticed and the area");
    assert.ok(await evaluate(`!!${q(`${item} [data-slot="event-log-new"]`)}`), "the fresh row is marked new");
    await until(async () => await rows() === before + 2 && await evaluate(`${q(item)}.dataset.level === 'info' && ${q(item)}.textContent.includes('Responding again')`), "the next good poll logs a recovery above it", 9000);
    assert.equal(await evaluate(`document.querySelectorAll('${item} [data-slot="event-log-new"]').length`), 2, "only the two rows that arrived since the last look are marked new");
    // Search narrows what is listed, across messages and areas.
    await type(`${panel} [aria-label="Search stored logs"]`, "index refused");
    await until(async () => await rows() === 1 && await evaluate(`${q(item)}.textContent.includes('Fixture index refused')`), "search keeps the matching row");
    await type(`${panel} [aria-label="Search stored logs"]`, "no such entry anywhere");
    await until(() => evaluate(`${q(panel)}?.textContent.includes('Nothing to report')`), "a search with no match says so");
    await type(`${panel} [aria-label="Search stored logs"]`, "");
    await until(async () => await rows() === before + 2, "clearing the search lists everything again");
    await evaluate(`${q(panel)}.focus()`);
    await key("Escape");
    await closed("Escape closes the panel");
    assert.equal(await evaluate(`document.activeElement === ${q(button)}`), true, "focus returns to the rail button");
    assert.equal(await evaluate(`${q(button)}.dataset.unread`), undefined, "closing marks everything read");
    await click(button);
    await until(async () => await rows() === before + 2, "the panel reopens with every row");
    assert.equal(await evaluate(`document.querySelectorAll('${item} [data-slot="event-log-new"]').length`), 0, "nothing is new on a second look");
    await key("Escape");
    await closed("the panel closes again");
    const filed = requests.filter((request) => request.method === "POST" && request.path === "/api/app/diagnostics/report");
    assert.ok(filed.length >= 2, "each reported event is also filed with the machine's stored log");
  });

  await check("a return visit whose list cannot be read keeps the saved list and conversation on screen", async () => {
    // Placed after the scenario that counts the event log's rows: this failure and recovery repeat its events.
    await open(main, "Mock main answer");
    const rows = await evaluate(`document.querySelectorAll('[data-slot="app-conversation-link"]').length`);
    const logged = (message, since) => evaluate(`JSON.parse(sessionStorage.getItem('adi.event-log.v1') ?? '{"events":[]}').events.some(event => event.message === ${JSON.stringify(message)} && event.lastSeen >= ${since})`);
    const since = await evaluate("Date.now()");
    await evaluate("window.__beforeFailedVisit = true");
    await send("Page.reload");
    // From here on only the new document can meet the failure; which of its reads does is not fixed.
    failEveryIndex = true;
    await until(async () => await evaluate("!window.__beforeFailedVisit") && await contains("Mock main answer"), "the saved conversation is on screen");
    await until(() => logged("Fixture index refused", since), "the failed list read is in the event log", 9000);
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="app-conversation-link"]').length`), rows, "the saved rows stay listed");
    assert.equal(await evaluate(`!!document.querySelector('[data-slot="app-chat-sidebar"] [role="alert"]')`), false, "the list shows no banner");
    assert.equal(await evaluate(`${q('nav[aria-label="Conversations"]')}.textContent.includes('unavailable')`), false);
    assert.ok(await contains("Mock main answer"));
    failEveryIndex = false;
    await until(() => logged("Responding again.", since), "the next poll recovers", 9000);
  });

  await check("mobile navigation, keyboard dismissal and both themes keep chat within the viewport", async () => {
    await open(main, "Mock main answer");
    for (const mode of ["light", "dark", "mobile"]) {
      const mobile = mode === "mobile";
      await setTheme(mode === "dark");
      await send("Emulation.setDeviceMetricsOverride", { width: mobile ? 390 : 1280, height: 900, deviceScaleFactor: 1, mobile });
      if (mobile) {
        assert.equal(await evaluate(`getComputedStyle(${q('[data-slot="app-sidebar"]')}).display`), "none");
        const bounds = await evaluate(`(() => {const r=${q(chat)}.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height}})()`);
        assert.deepEqual(bounds, {x:0,y:0,width:390,height:900});
        await click('[aria-label="Back to chats"]');
        await until(() => evaluate(`location.search === "" && !!document.querySelector('[data-mobile-screen="chats"]')`), "back opens inbox");
        assert.equal(await evaluate(`getComputedStyle(${q('[data-slot="app-content"]')}).display`), "none");
        assert.equal(await evaluate(`getComputedStyle(${q('[data-slot="app-topbar"]')}).display`), "none", "inbox uses its own header instead of the outer topbar");
        assert.equal(await evaluate(`${q('[data-slot="app-chat-sidebar"]')}.getBoundingClientRect().top < 20`), true);
        assert.equal(await evaluate(`${q('[data-slot="app-chat-sidebar"] nav')}.getBoundingClientRect().height > 400`), true);
        const tabs = await evaluate(`Array.from(document.querySelectorAll('[data-slot="global-navigation"] a, [data-slot="global-navigation"] button')).map(a => {const r=a.getBoundingClientRect();return {width:r.width,bottom:r.bottom};})`);
        assert.equal(tabs.length, 5, "Home, Chats, Marketplace, Events and Settings share the tab bar");
        assert.ok(tabs.every(tab => tab.width >= 64 && Math.abs(tab.bottom - 900) < 1), JSON.stringify(tabs));
        await click('[aria-label="Filter conversations"]');
        await key("Escape");
        assert.equal(await evaluate(`document.querySelector('[data-slot="app-chat-menu"]') === null`), true);
        await open(other, "Mock other answer");
      }
      await until(() => evaluate(`/^oklch\\(/.test(getComputedStyle(${q(`${chat} [data-slot="chat-composer"]`)}).backgroundColor)`), "chat theme tokens apply");
      const colors = await evaluate(`(() => {
        const context = document.createElement('canvas').getContext('2d', {willReadFrequently:true});
        const rgba = color => {
          context.clearRect(0,0,1,1);
          context.fillStyle=color;
          context.fillRect(0,0,1,1);
          return [...context.getImageData(0,0,1,1).data];
        };
        const sample = (selector, property) => rgba(getComputedStyle(document.querySelector(selector))[property]);
        const tokens = getComputedStyle(document.documentElement);
        return {
          surface: sample('[data-slot="app-layout-surface"]','backgroundColor'),
          sidebar: sample('[data-slot="app-secondary-navigation"]','backgroundColor'),
          foreground: sample('${chat}','color'),
          composer: sample('${chat} [data-slot="chat-composer"]','backgroundColor'),
          input: sample('${composer}','color'),
          userBackground: sample('${chat} [data-from="user"] [data-slot="chat-message-content"]','backgroundColor'),
          userForeground: sample('${chat} [data-from="user"] [data-slot="chat-message-content"]','color'),
          assistantBackground: sample('${chat} [data-from="assistant"] [data-slot="chat-message-content"]','backgroundColor'),
          assistantForeground: sample('${chat} [data-from="assistant"] [data-slot="chat-message-content"]','color'),
          backgroundToken: rgba(tokens.getPropertyValue('--background')),
          foregroundToken: rgba(tokens.getPropertyValue('--foreground')),
          mutedToken: rgba(tokens.getPropertyValue('--muted')),
        };
      })()`);
      assert.deepEqual(colors.surface, mode === "dark" ? [0,0,0,255] : [255,255,255,255], `${mode} chat surface uses the selected white or black theme`);
      assert.ok(colors.sidebar[3] > 0, `${mode} sidebar has a visible background`);
      assert.ok(colors.sidebar.slice(0,3).every(value => mode === "dark" ? value > 0 && value < 40 : value > 230 && value < 255), `${mode} sidebar stays subtly separate from the chat surface`);
      assert.deepEqual(colors.composer, colors.backgroundToken, `${mode} composer follows the global background`);
      assert.deepEqual(colors.userBackground, colors.mutedToken, `${mode} user message uses the selected muted bubble`);
      assert.equal(colors.assistantBackground[3], 0, `${mode} assistant message stays unboxed`);
      for (const key of ["foreground", "input", "userForeground", "assistantForeground"]) {
        assert.deepEqual(colors[key], colors.foregroundToken, `${mode} ${key} follows the global foreground`);
      }
      assert.ok(colors.foreground.slice(0,3).every(value => mode === "dark" ? value > 230 : value < 50), `${mode} text contrasts with the chat surface`);
      assert.equal(await evaluate("localStorage.getItem('design-system-theme')"), mode === "dark" ? "dark" : "light", `${mode} global theme choice is persisted`);
      assert.equal(await evaluate("document.documentElement.scrollWidth <= innerWidth"), true, `${mode} has no horizontal page overflow`);
      const bounds = await evaluate(`(() => { const r=${q(`${chat} [data-slot="chat-thread-viewport"]`)}.getBoundingClientRect(); return {width:r.width,height:r.height,left:r.left,right:r.right}; })()`);
      assert.ok(bounds.width > 100 && bounds.height > 100, `${mode} transcript has useful visible dimensions`);
      assert.ok(bounds.left >= 0 && bounds.right <= (mobile ? 390 : 1280) + 1, `${mode} transcript fits the viewport`);
      if (process.env.BROWSER_SCREENSHOTS_DIR) {
        await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
        await evaluate("window.scrollTo({top:0,behavior:'instant'})");
        await pause(150);
        const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
        await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, `design-system-app-chat-${mode}.png`), Buffer.from(screenshot.data, "base64"));
      }
    }
  });

  await check("mobile starts in the inbox and keeps apps, marketplace and new chat reachable", async () => {
    await send("Emulation.setDeviceMetricsOverride", {width:390,height:844,deviceScaleFactor:1,mobile:true});
    await send("Page.navigate", {url: origin + "/"});
    await until(() => evaluate(`location.pathname === '/interface' && location.search === '' && !!document.querySelector('[data-mobile-screen="chats"]')`), "fresh mobile visit opens inbox");
    await until(() => evaluate(`${q(listLink(main))} !== null`), "inbox loads conversations");
    await click('[aria-label="ADI home"]');
    await until(() => evaluate(`location.pathname === '/' && !!document.querySelector('main[aria-label="Home"]')`), "Home tab opens apps");
    await click('[data-slot="global-navigation"] a[href="/marketplace"]');
    await until(() => evaluate(`location.pathname === '/marketplace' && !!document.querySelector('main[aria-label="Marketplace"]')`), "Marketplace tab opens");
    await click('[data-slot="global-navigation"] a[href="/interface"]');
    await until(() => evaluate(`location.pathname === '/interface' && location.search === '' && !!document.querySelector('[data-mobile-screen="chats"] [data-slot="app-chat-sidebar"]')`), "Chats tab opens inbox");
    assert.equal(await evaluate(`document.querySelector('main[aria-label="Marketplace"]') === null`), true, "the previous page is gone once the tab route changes");
    assert.ok(await evaluate(`${q('[data-slot="global-navigation"] a[aria-label="Settings"]')}.getAttribute('href') === '/settings'`));
    if (process.env.BROWSER_SCREENSHOTS_DIR) {
      await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, {recursive:true});
      const shot = await send("Page.captureScreenshot", {format:"png",captureBeyondViewport:false});
      await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR,"mobile-inbox.png"),Buffer.from(shot.data,"base64"));
    }
    await click('[aria-label="New conversation"]');
    await until(() => evaluate(`new URLSearchParams(location.search).has('agent') && !!document.querySelector('[data-mobile-screen="conversation"]')`), "new conversation opens full screen");
    await click('[aria-label="Back to chats"]');
    await until(() => evaluate(`location.search === ''`), "new conversation returns to inbox");
    await open(main, "Mock main answer");
    await evaluate("window.__beforeMobileReload = true");
    await send("Page.reload");
    await until(async () => await evaluate("!window.__beforeMobileReload") && await contains("Mock main answer"), "mobile conversation deep link survives reload");
    assert.equal(await evaluate(`!!document.querySelector('[data-mobile-screen="conversation"]')`), true);
  });

  assert.deepEqual(mockErrors, [], "all API requests stayed within the intercepted fixture contract");
  assert.deepEqual(errors, [], "browser exceptions and console errors");
  console.log(`\n${checks.length} connected-chat browser scenarios passed; all API requests intercepted; no exceptions or console errors.`);
} catch (error) {
  console.error(`Connected-chat browser check failed after ${checks.length} completed scenarios.`);
  if (errors.length) console.error(`Browser errors: ${errors.join("\n")}`);
  if (mockErrors.length) console.error(`Fixture errors: ${mockErrors.join("\n")}`);
  console.error(`Intercepted request counts: ${JSON.stringify(Object.fromEntries([...new Set(requests.map((item) => `${item.method} ${item.path}`))].map((key) => [key, requests.filter((item) => `${item.method} ${item.path}` === key).length])))}`);
  if (evaluate && socket?.readyState === WebSocket.OPEN) {
    try {
      // Diagnostics deliberately omit conversation text and request bodies.
      console.error(JSON.stringify(await evaluate(`(() => ({url:location.pathname+location.search,title:document.title,slots:Object.fromEntries(['app-chat','app-chat-sidebar','chat-composer','chat-thread','app-chat-question'].map(slot=>[slot,document.querySelectorAll('[data-slot="'+slot+'"]').length])),viteError:!!document.querySelector('vite-error-overlay')}))()`)));
      if (process.env.BROWSER_SCREENSHOTS_DIR) {
        await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
        const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
        await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, "design-system-app-chat-failure.png"), Buffer.from(screenshot.data, "base64"));
      }
    } catch (diagnosticError) { console.error(`Could not collect browser diagnostics: ${diagnosticError.message}`); }
  }
  throw error;
} finally {
  for (const held of [heldPeek, heldReply, heldStart]) {
    try { await held?.release(); } catch { /* The browser may already be closed. */ }
  }
  socket?.close();
  chrome.kill();
  await chrome.exited;
  await rm(profile, { recursive: true, force: true });
}
