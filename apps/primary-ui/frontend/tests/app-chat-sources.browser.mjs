/**
 * Source, filter, and grouping regressions against the live React UI.
 * Every app API request is intercepted before navigation and fulfilled from fixtures.
 * This script never forwards a request to app.adi or a paired node.
 * Run: bun frontend/tests/app-chat-sources.browser.mjs
 */
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

const origin = new URL(process.env.SHOWCASE_URL ?? "http://design-system.adi").origin;
const apiBase = process.env.APP_API_BASE ?? "/api/app";
const fixturePath = (pathname) => apiBase === "/api" && pathname.startsWith("/api/") ? `/api/app${pathname.slice(4)}` : pathname;
const chromePath = process.env.CHROME_BIN ?? (process.platform === "darwin"
  ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : "google-chrome");
const profile = await mkdtemp(resolve(tmpdir(), "design-system-chat-sources-"));
const chrome = Bun.spawn([
  chromePath, "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`,
  "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "about:blank",
], { stdout: "ignore", stderr: "ignore" });

const remote = "laptop-b";
const main = { agent: "design-assistant", runId: "shared-run" };
const remoteMain = { ...main, source: remote };
const child = { agent: "layout-reviewer", runId: "child-run", source: remote };
const question = { agent: "layout-reviewer", runId: "question-run", source: remote };
const snapshots = new Map();
const metadata = new Map();
const requests = [];
const errors = [];
const mockErrors = [];
const checks = [];
const capabilities = { answerable: true, interactive: false, history: true, images: false, tool_steps: true };
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const idOf = (ref) => JSON.stringify([ref.source ?? "", ref.agent, ref.runId]);
const copy = (value) => structuredClone(value);
const localName = (source) => source ?? "local";
const conversationUrl = (ref) => `${origin}/interface?${new URLSearchParams({ agent: ref.agent, run: ref.runId, ...(ref.source ? { node: ref.source } : {}) })}`;
const row = (ref) => `[data-slot="app-conversation-link"][data-source="${ref.source ?? ""}"][data-agent="${ref.agent}"][data-run="${ref.runId}"]`;
const composer = '[data-slot="app-chat"] [data-slot="chat-composer-input"]';
const chat = '[data-slot="app-chat"]';
const q = (selector) => `document.querySelector(${JSON.stringify(selector)})`;
let socket;
let send;
let evaluate;
let holdPeek = null;
let heldPeek = null;
let holdReply = null;
let heldReply = null;
let created = 0;
let failImageReply = false;
let imageSequence = 0;
const uploadedImages = new Map();
const pngBase64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a9X0AAAAASUVORK5CYII=";
let ignoreRemoteFilters = false;

function turn(role, text, seq, steps = []) {
  return { role, text, seq, at: 1000 + seq, pending: false, queued: false, images: [], steps };
}
function put(ref, title, options = {}) {
  const sourceName = localName(ref.source);
  const steps = ref.runId === main.runId ? [
    { kind: "calls", from: 0, to: 1, count: 1, tools: ["functions.exec_command"], preview: "Read source fixture", status: "ok" },
    ...(ref.source ? [{ kind: "tool", name: "Bash", input: JSON.stringify({ command: "adi-mono agents run layout-reviewer 'Check remote source'" }), status: "ok", output: "Started agent layout-reviewer as background process 1234.\n  run: child-run\n" }] : []),
  ] : [];
  snapshots.set(idOf(ref), {
    name: ref.agent, run_id: ref.runId, running: false, answerable: true, interactive: false,
    caps: capabilities, cwd: `/fixture/${sourceName}`, output: "",
    turns: [turn("user", `${sourceName} fixture request`, 0), turn("assistant", `${sourceName} fixture answer ${ref.runId}`, 1, steps)],
    total_turns: 2, awaits: [], ...options.snapshot,
  });
  metadata.set(idOf(ref), {
    ...ref, title, launched_by: ref.source ? "human:adi" : "human", hidden: false, starred: false,
    last_activity: options.activity ?? 5000, ...options,
  });
}
for (const source of [undefined, remote]) {
  const ref = (agent, runId) => ({ agent, runId, ...(source ? { source } : {}) });
  put(ref(main.agent, main.runId), `${localName(source)} shared chat`);
  put(ref(main.agent, "other-human"), `${localName(source)} other human`, { launched_by: "human:teammate", activity: 4900 });
  put(ref(main.agent, "agent-started"), `${localName(source)} agent started`, { launched_by: "agent:orchestrator", activity: 4800 });
  put(ref("layout-reviewer", "run-starred-only"), `${localName(source)} run star is not agent star`, { starred: true, activity: 4700 });
  put(ref(main.agent, "hidden-run"), `${localName(source)} hidden chat`, { hidden: true, activity: 4600 });
  for (let index = 0; index < 4; index++) put(ref(main.agent, `project-${index}`), `${localName(source)} project session ${index}`, { activity: 4500 - index });
}
put(child, "Remote child chat", { launched_by: "agent:design-assistant", activity: 5100 });
put(question, "Remote question chat", { launched_by: "agent:orchestrator", activity: 5200, snapshot: {
  pending_question: { id: "remote-ask", headline: "Remote decision", note: "Choose an answer", asked_at: 1000, questions: [{ header: "Decision", question: "What should happen?", options: [], multi_select: false }] },
} });

function agents(source) {
  return { agents: [
    { name: main.agent, starred: true, project: "design", runnable: true, running: false, caps: capabilities },
    { name: "layout-reviewer", starred: false, project: "review", runnable: true, running: false, caps: capabilities },
  ], source: localName(source) };
}
function indexFor(source, params) {
  assert.ok(["false", "true"].includes(params.get("hidden")), "index explicitly chooses visible or hidden sessions");
  const filter = params.get("filter") ?? "all";
  assert.ok(["all", "starred", "mine", "me"].includes(filter), "known server filter");
  const hidden = params.get("hidden") === "true";
  let items = [...metadata.values()].filter((item) => item.source === source && item.hidden === hidden);
  items = items.filter((item) => {
    const snapshot = snapshots.get(idOf(item));
    if (source === remote && ignoreRemoteFilters) return true;
    if (snapshot.pending_question || snapshot.awaits.length) return true;
    return filter === "all" || filter === "starred" && item.agent === main.agent ||
      filter === "mine" && item.launched_by.startsWith("human") ||
      filter === "me" && item.launched_by === (source ? "human:adi" : "human");
  }).sort((a, b) => b.last_activity - a.last_activity);
  const total = items.length;
  const groups = [];
  if (params.get("group") === "project" && !hidden) {
    assert.equal(params.get("group_limit"), "3");
    const more = Object.fromEntries((params.get("group_more") ?? "").split(",").filter(Boolean).map((pair) => pair.split(":")));
    const grouped = [];
    for (const [project, label, names] of [["design", "Design project", [main.agent]], ["review", "Review project", ["layout-reviewer"]]]) {
      const matches = items.filter((item) => names.includes(item.agent));
      groups.push({ project, label, agents: names, total: matches.length });
      grouped.push(...matches.slice(0, Number(params.get("group_limit")) + Number(more[project] ?? 0)));
    }
    items = grouped;
  }
  return {
    agents: agents(source).agents.map(({ starred: _starred, ...agent }) => ({ ...agent, answerable: true, interactive: false,
      runs: items.filter((item) => item.agent === agent.name).map((item) => {
        const snapshot = snapshots.get(idOf(item));
        const title = source === remote && ignoreRemoteFilters ? `${item.title} legacy server` : item.title;
        return { run_id: item.runId, title, message: title, caps: capabilities,
          started_at: item.last_activity, last_activity: item.last_activity, launched_by: item.launched_by,
          hidden: item.hidden, starred: item.starred, running: snapshot.running,
          awaits: snapshot.awaits, ...(snapshot.pending_question ? { pending_question: snapshot.pending_question } : {}) };
      }),
    })), total, asking_user: source ? "adi" : null, groups,
  };
}
async function until(fn, description, timeout = 8000) {
  let last;
  const deadline = Date.now() + timeout;
  do {
    try { const value = await fn(); if (value) return value; } catch (error) { last = error; }
    await pause(25);
  } while (Date.now() < deadline);
  throw new Error(`Timed out: ${description}${last ? ` (${last.message})` : ""}`);
}
async function fulfill(requestId, data) {
  try {
    await send("Fetch.fulfillRequest", { requestId, responseCode: 200,
      responseHeaders: [{ name: "Content-Type", value: "application/json" }, { name: "Cache-Control", value: "no-store" }],
      body: Buffer.from(JSON.stringify(data)).toString("base64") });
  } catch (error) {
    if (!/Invalid InterceptionId|Invalid interceptionId|Invalid requestId|No resource with given identifier|interception.*not found/i.test(error.message)) throw error;
  }
}
async function intercept({ requestId, request }) {
  try {
    const url = new URL(request.url);
    url.pathname = fixturePath(url.pathname);
    const body = request.postData && !url.pathname.endsWith("/agents/attachment") ? JSON.parse(request.postData) : {};
    const remotePath = url.pathname.match(/^\/api\/app\/node\/([^/]+)\/api(\/.*)$/);
    const source = remotePath ? decodeURIComponent(remotePath[1]) : undefined;
    const path = remotePath ? remotePath[2] : url.pathname.replace(/^\/api\/app/, "");
    requests.push({ method: request.method, path, source, query: url.search, body });
    if (!url.pathname.startsWith("/api/app/")) throw new Error(`Unexpected direct API request: ${request.method} ${url.pathname}`);
    if (source !== undefined && source !== remote) throw new Error(`Unexpected fixture source: ${source}`);
    if (request.method === "POST" && path === "/agents/attachment") {
      const image = { id: `${localName(source)}-image-${++imageSequence}`, name: "shot.png", media_type: request.headers["Content-Type"] || request.headers["content-type"] || "image/png", size: 68 };
      uploadedImages.set(image.id, { ...image, source });
      return await fulfill(requestId, image);
    }
    if (request.method === "GET" && path.startsWith("/agents/attachment/")) {
      const image = uploadedImages.get(path.split("/").at(-1));
      assert.ok(image && image.source === source, "image preview stays on its owning machine");
      return await send("Fetch.fulfillRequest", { requestId, responseCode: 200, responseHeaders: [{ name: "Content-Type", value: "image/png" }], body: pngBase64 });
    }
    if (request.method === "GET" && path === "/voice") return await fulfill(requestId, { engines: [{id:"browser",label:"Browser",ready:true,in_browser:true,detail:"Fixture"}],default_engine:"browser" });
    if (request.method === "POST" && path === "/agents/run/rename") {
      const item = metadata.get(idOf({ agent: body.name, runId: body.run_id, ...(source ? {source} : {}) }));
      assert.ok(item, "renaming an existing source conversation");
      item.title = body.title || "Default fixture title";
      return await fulfill(requestId, {agents:[]});
    }
    if (request.method === "GET" && path === "/fleet/nodes") return await fulfill(requestId, { nodes: [{ node: remote, locked: false }, { node: "locked-node", locked: true }] });
    if (request.method === "GET" && path === "/agents") return await fulfill(requestId, agents(source));
    if (request.method === "GET" && path === "/agents/runs/all") return await fulfill(requestId, indexFor(source, url.searchParams));
    const ref = { agent: body.name, runId: body.run_id, ...(source ? { source } : {}) };
    const snapshot = snapshots.get(idOf(ref));
    if (request.method === "POST" && path === "/agents/run/peek" && snapshot) {
      const response = copy(snapshot);
      if (holdPeek === idOf(ref)) { holdPeek = null; heldPeek = { release: () => fulfill(requestId, response) }; return; }
      return await fulfill(requestId, response);
    }
    if (request.method === "POST" && path === "/agents/run/steps" && snapshot) {
      assert.equal(body.turn, 1);
      assert.equal(body.from, 0);
      assert.equal(body.to, 1);
      return await fulfill(requestId, { steps: [{ kind: "tool", name: "functions.exec_command", input: JSON.stringify({ cmd: `printf ${localName(source)}-fixture` }), status: "ok", output: `${localName(source)} source tool output` }] });
    }
    if (request.method === "POST" && path === "/agents/run/reply" && snapshot) {
      if (failImageReply) { failImageReply = false; return await fulfill(requestId, { ok: false, error: "Fixture image reply refused" }); }
      const next = turn("user", body.message, snapshot.turns.length);
      next.images = (body.attachments ?? []).map((id) => {
        const image = uploadedImages.get(id);
        assert.ok(image && image.source === source, "attachment IDs belong to the conversation source");
        return image;
      });
      snapshot.turns.push(next, turn("assistant", `${localName(source)} reply accepted`, snapshot.turns.length + 1));
      snapshot.total_turns = snapshot.turns.length;
      const response = copy(snapshot);
      if (holdReply === idOf(ref)) { holdReply = null; heldReply = { release: () => fulfill(requestId, response) }; return; }
      return await fulfill(requestId, response);
    }
    if (request.method === "POST" && path === "/agents/run") {
      const ref = { agent: body.name, runId: `new-source-${++created}`, ...(source ? { source } : {}) };
      put(ref, `${localName(source)} newly started chat`);
      return await fulfill(requestId, { run_id: ref.runId, message: "Started fixture", state: { agents: [] } });
    }
    if (request.method === "POST" && path === "/agents/run/stop" && snapshot) {
      snapshot.running = false;
      return await fulfill(requestId, { name: ref.agent, runs: [] });
    }
    if (request.method === "POST" && path === "/agents/run/answer" && snapshot) {
      assert.equal(body.ask, snapshot.pending_question?.id);
      delete snapshot.pending_question;
      snapshot.turns.push(turn("user", "remote question answered", snapshot.turns.length));
      snapshot.total_turns = snapshot.turns.length;
      return await fulfill(requestId, copy(snapshot));
    }
    throw new Error(`Unimplemented intercepted API route: ${localName(source)} ${request.method} ${path}`);
  } catch (error) {
    mockErrors.push(error.message);
    await fulfill(requestId, { ok: false, error: "This request is unavailable in the source fixture." });
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
      pending.delete(message.id); clearTimeout(waiting.timer);
      message.error ? waiting.reject(new Error(JSON.stringify(message.error))) : waiting.resolve(message.result);
    }
    if (message.method === "Fetch.requestPaused") void intercept(message.params).catch((error) => mockErrors.push(error.message));
    if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text);
    if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") errors.push(message.params.args.map((item) => item.value ?? item.description).join(" "));
    if (message.method === "Log.entryAdded" && message.params.entry.level === "error") errors.push(message.params.entry.text);
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
    const code = { Enter: 13, Escape: 27, Backspace: 8, ArrowDown: 40, ArrowUp: 38, Home: 36, End: 35 }[key];
    await send("Input.dispatchKeyEvent", { type: "keyDown", key, code: key, windowsVirtualKeyCode: code, ...(key === "Enter" ? { text: "\r" } : {}) });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key, code: key, windowsVirtualKeyCode: code });
  }
  async function type(selector, text) {
    await click(selector); await evaluate(`${q(selector)}.select()`); await key("Backspace");
    if (text) await send("Input.insertText", { text });
    await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  }
  const contains = (text) => evaluate(`${q(`${chat} [data-slot="chat-thread"]`)}?.textContent.includes(${JSON.stringify(text)})===true`);
  const draft = () => evaluate(`${q(composer)}?.value`);
  const activeIs = (ref) => evaluate(`(() => {const p=new URLSearchParams(location.search);return p.get('agent')===${JSON.stringify(ref.agent)}&&p.get('run')===${JSON.stringify(ref.runId)}&&(p.get('node')??'')===${JSON.stringify(ref.source ?? "")};})()`);
  async function open(ref) {
    await until(() => evaluate(`!!${q(row(ref))}`), "source conversation row");
    await click(row(ref)); await until(() => activeIs(ref), "source-aware route");
    await until(() => contains(`${localName(ref.source)} fixture answer ${ref.runId}`), "source-aware transcript");
  }
  async function option(role, label) {
    const selector = await evaluate(`(() => { const node=[...document.querySelectorAll('[role="${role}"]')].find(node=>node.textContent.trim()===${JSON.stringify(label)});if(!node)throw new Error('Missing menu option');node.dataset.fixtureTarget='true';return '[data-fixture-target="true"]';})()`);
    await click(selector);
    await evaluate("document.querySelector('[data-fixture-target]')?.removeAttribute('data-fixture-target')");
  }
  async function settleIndex() {
    await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
    await until(() => evaluate(`${q('[aria-label="Conversations"]')}.getAttribute('aria-busy')==='false'`), "source indexes settled");
  }
  async function filter(label) { await click('[aria-label="Filter conversations"]'); await option("menuitemradio", label); await settleIndex(); }
  async function group(label) { await click('[aria-label="Filter conversations"]'); await option("menuitemradio", label); await settleIndex(); }
  async function check(name, fn) { await fn(); checks.push(name); console.log(`PASS ${name}`); }
  async function screenshot(name) {
    if (!process.env.BROWSER_SCREENSHOTS_DIR) return;
    await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
    const capture = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
    await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, `design-system-sources-${name}.png`), Buffer.from(capture.data, "base64"));
  }
  async function reloadWhen(expression, description) {
    const previous = await evaluate("performance.timeOrigin");
    await send("Page.reload");
    await until(() => evaluate(`performance.timeOrigin!==${previous}&&(${expression})`), description, 15000);
  }
  await Promise.all([send("Runtime.enable"), send("Page.enable"), send("Log.enable")]);
  // No continueRequest path exists: even unexpected direct API calls fail closed in the fixture.
  await send("Fetch.enable", { patterns: [
    { urlPattern: "*/api/*", requestStage: "Request" },
    { urlPattern: "*://app.adi/api/*", requestStage: "Request" },
  ] });
  await send("Page.addScriptToEvaluateOnNewDocument", { source: `if(!localStorage.getItem('source-fixture-seeded')){localStorage.setItem('adi-session-sources',JSON.stringify({local:true,nodes:['${remote}']}));localStorage.setItem('adi-session-group',JSON.stringify('machine'));localStorage.setItem('source-fixture-seeded','true');}` });
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
  await send("Page.navigate", { url: conversationUrl(main) });
  await until(() => contains("local fixture answer shared-run"), "source fixtures render", 15000);

  await check("sidebar activity time and machine metadata follow source selection and grouping", async () => {
    await until(() => evaluate(`!!${q(row(main))}&&!!${q(row(remoteMain))}`), "both source rows");
    assert.equal(await evaluate(`${q(row(main))}.querySelector('time').dateTime`), new Date(5000).toISOString());
    assert.match(await evaluate(`${q(row(main))}.querySelector('time').textContent`), /ago$/);
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="app-conversation-machine"]').length`), 0, "machine headings replace row labels");
    await filter("Disabled");
    assert.equal(await evaluate(`${q(row(main))}.querySelector('[data-slot="app-conversation-machine"]').textContent.trim()`), "· This machine");
    assert.equal(await evaluate(`${q(row(remoteMain))}.querySelector('[data-slot="app-conversation-machine"]').textContent.trim()`), `· ${remote}`);
    await click('[aria-label="Conversation sources"]');
    await option("menuitemcheckbox", "This machine");
    await key("Escape");
    await until(() => evaluate(`!${q(row(main))}`), "remote source only");
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="app-conversation-machine"]').length`), 0, "one remote machine needs no row label");
    await click('[aria-label="Conversation sources"]');
    await option("menuitemcheckbox", "This machine");
    await key("Escape");
    await filter("Machine");
    await until(() => evaluate(`!!${q(row(main))}`), "local source restored");
  });

  await check("default me filter requests both sources and preserves pending questions", async () => {
    await until(() => evaluate(`!!${q(row(remoteMain))}&&!!${q(row(question))}`), "remote index and pending question render");
    for (const source of [undefined, remote]) {
      assert.ok(requests.some((request) => request.source === source && request.path === "/agents/runs/all" && new URLSearchParams(request.query).get("filter") === "me"));
      assert.equal(await evaluate(`!!${q(row({ ...main, source, runId: "other-human" }))}`), false);
      assert.equal(await evaluate(`!!${q(row({ ...main, source, runId: "hidden-run" }))}`), false);
    }
    assert.equal(await evaluate(`${q(row(remoteMain))}.getAttribute('href')`), `/interface?agent=${remoteMain.agent}&run=${remoteMain.runId}&node=${remote}`);
  });

  await check("new chat includes starred agents from every selected machine", async () => {
    async function visit(path) {
      await send('Page.navigate', {url: `${origin}${path}`});
      await until(() => evaluate(`!!document.querySelector('[data-slot="app-chat"]')`), 'chat route loaded');
    }
    await visit('/chat');
    await until(() => evaluate(`document.querySelectorAll('[data-slot="new-chat-starred-agent"]').length===2`), "both sources have starred choices");
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-slot="new-chat-starred-agent"]')).map(el=>el.dataset.source).sort()`), ['', remote].sort());
    await screenshot('new-chat-starred');
    const localCard = '[data-slot="new-chat-starred-agent"][data-source=""]';
    const remoteCard = '[data-slot="new-chat-starred-agent"][data-source="' + remote + '"]';
    const mutations = requests.filter((request) => request.method === "POST").length;
    await evaluate(`${q(localCard)}.dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, cancelable:true, clientX:200, clientY:200}))`);
    await until(() => evaluate(`!!document.querySelector('[data-slot="agent-context-menu"]')`), 'agent context menu');
    await click('[data-slot="agent-context-menu"] button');
    await until(() => evaluate(`!${q(localCard)} && !!${q(remoteCard)}`), 'personal unstar isolates the machine');
    const beforeReload = await evaluate('performance.timeOrigin');
    await send('Page.reload');
    await until(() => evaluate(`performance.timeOrigin!==${beforeReload} && !${q(localCard)} && !!${q(remoteCard)} && !!document.querySelector('details summary')`), 'personal unstar persists after reload', 30000);
    await click('details summary');
    await click('details button');
    await until(() => evaluate(`!!document.querySelector('[data-slot="agent-context-menu"]')`), 'restore action available');
    await click('[data-slot="agent-context-menu"] button');
    await until(() => evaluate(`!!${q(localCard)}`), 'restore inherits shared star');
    assert.equal(requests.filter((request) => request.method === "POST").length, mutations);

    async function renamePersonal(value) {
      await evaluate(`${q(localCard)}.dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, cancelable:true, clientX:200, clientY:200}))`);
      await click('[data-slot="agent-context-menu"] button:nth-child(2)');
      await evaluate(`(() => { const input = document.querySelector('input[aria-label="Name for me"]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(value)}); input.dispatchEvent(new Event('input', {bubbles:true})); })()`);
      await click('[data-slot="agent-context-menu"] button[type="submit"]');
    }
    await renamePersonal('My personal helper');
    await until(() => evaluate(`${q(localCard)}.textContent.includes('My personal helper') && !${q(remoteCard)}.textContent.includes('My personal helper')`), 'personal name isolates machine');
    await reloadWhen(`${q(localCard)}?.textContent.includes('My personal helper')`, 'personal name survives reload');
    await renamePersonal('');
    await until(() => evaluate(`!${q(localCard)}.textContent.includes('My personal helper')`), 'empty name restores shared name');
    assert.equal(requests.filter((request) => request.method === "POST").length, mutations);

    const launches = requests.filter((request) => request.path === '/agents/run').length;
    await click('[data-slot="new-chat-starred-agent"][data-source="' + remote + '"]');
    await until(() => evaluate(`new URLSearchParams(location.search).get('node')===${JSON.stringify(remote)}`), "remote starred agent selected");
    assert.equal(await evaluate(`new URLSearchParams(location.search).has('run')`), false);
    assert.equal(requests.filter((request) => request.path === '/agents/run').length, launches);
    await visit(`/chat/${main.agent}/${main.runId}`);
    await until(() => contains('local fixture answer shared-run'), 'original conversation restored');
  });

  await check("legacy chat bookmarks retain their machine, agent and explicit run selection", async () => {
    async function visit(path) {
      const previous = await evaluate("performance.timeOrigin");
      await send("Page.navigate", { url: `${origin}${path}` });
      await until(() => evaluate(`performance.timeOrigin!==${previous}&&location.pathname==='/interface'&&!!${q(chat)}`), "legacy bookmark reaches React chat", 15000);
    }
    for (const ref of [main, remoteMain]) {
      await visit(`/chat/${ref.source ? `@${ref.source}/` : ""}${ref.agent}/${ref.runId}`);
      await until(() => activeIs(ref), "legacy bookmark selects its exact conversation");
      await until(() => contains(`${localName(ref.source)} fixture answer ${ref.runId}`), "legacy bookmark loads its owning transcript");
      assert.ok(requests.some((request) => request.path === "/agents/run/peek" && request.source === ref.source && request.body.name === ref.agent && request.body.run_id === ref.runId));
    }
    const peeksBefore = requests.filter((request) => request.path === "/agents/run/peek").length;
    const launchesBefore = requests.filter((request) => request.path === "/agents/run").length;
    for (const [path, agent, source] of [
      ["/chat/layout-reviewer", "layout-reviewer", undefined],
      [`/chat/@${remote}/layout-reviewer`, "layout-reviewer", remote],
      [`/chat/@${remote}`, undefined, remote],
      ["/chat", undefined, undefined],
    ]) {
      await visit(path);
      await until(() => evaluate(`${q(`${chat} [data-slot="app-chat-agent"]`)}!==null&&${q(composer)}?.disabled===false`), "legacy selection shows a ready new-chat form");
      await settleIndex();
      const params = await evaluate("Object.fromEntries(new URLSearchParams(location.search))");
      assert.equal(params.node, source);
      assert.equal(params.agent, agent);
      assert.equal(params.run, "");
      assert.equal(await evaluate(`!!${q(`${chat} [data-slot="chat-thread"]`)}`), false);
      if (agent) assert.equal(await evaluate(`${q('[data-app-chat-agent]')}.value`), agent);
    }
    assert.equal(requests.filter((request) => request.path === "/agents/run/peek").length, peeksBefore, "partial bookmarks never resume a conversation");
    assert.equal(requests.filter((request) => request.path === "/agents/run").length, launchesBefore, "bookmarks never launch a conversation");
    await visit(`/chat/${main.agent}/${main.runId}`);
    await until(() => contains("local fixture answer shared-run"), "local conversation restored after bookmark checks");
  });

  if (apiBase === "/api") await check("primary Settings and Fleet anchors remain on the current panel origin", async () => {
    const settings = await evaluate(`({raw:${q('[aria-label="Settings"]')}.getAttribute('href'),href:${q('[aria-label="Settings"]')}.href})`);
    assert.equal(settings.raw, "/extended/settings");
    assert.equal(new URL(settings.href).origin, origin);
    await click('[aria-label="Conversation sources"]');
    const fleet = await evaluate("(() => {const node=[...document.querySelectorAll('[role=menuitem]')].find(node=>node.textContent.trim()==='Manage sources in Fleet');return {raw:node.getAttribute('href'),href:node.href}})()");
    assert.equal(fleet.raw, "/extended/settings/fleet");
    assert.equal(new URL(fleet.href).origin, origin);
    await key("Escape");
  });

  await check("machine groups show five chats and support collapse and local pagination", async () => {
    const machine = `[data-slot="app-conversation-group"][aria-label="${remote}"]`;
    const toggle = `${machine} [data-slot="app-conversation-group-toggle"]`;
    const count = () => evaluate(`document.querySelectorAll('${machine} [data-slot="app-conversation-link"]').length`);
    assert.equal(await count(), 5);
    await click(toggle);
    assert.equal(await evaluate(`${q(toggle)}.getAttribute('aria-expanded')`), "false");
    assert.equal(await evaluate(`${q(machine + ' ul')}.checkVisibility()`), false);
    await key("Enter");
    assert.equal(await evaluate(`${q(toggle)}.getAttribute('aria-expanded')`), "true");
    await click(`${machine} [data-slot="app-conversation-group-more"]`);
    await until(async () => await count() > 5 && await count() <= 10, "next machine chats revealed");
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="app-conversation-group"][aria-label="This machine"] [data-slot="app-conversation-link"]').length`), 5, "other machine keeps its own page size");
  });

  await check("PDF picker, paste and drop upload as file links and send attachment IDs", async () => {
    await open(main);
    for (const method of ["picker", "paste", "drop"]) {
      await evaluate(`(() => {
        const dt = new DataTransfer(); dt.items.add(new File(['%PDF-1.7'], '${method}.pdf', {type:'application/pdf'}));
        const form = document.querySelector('[data-slot="chat-composer"]');
        const input = form.querySelector('input[type=file]');
        if (input.accept) throw new Error('File picker still restricts types');
        if ('${method}' === 'picker') {input.files=dt.files;input.dispatchEvent(new Event('change',{bubbles:true}));}
        else if ('${method}' === 'paste') form.dispatchEvent(new ClipboardEvent('paste',{clipboardData:dt,bubbles:true,cancelable:true}));
        else form.dispatchEvent(new DragEvent('drop',{dataTransfer:dt,bubbles:true,cancelable:true}));
      })()`);
      await until(() => evaluate(`document.querySelectorAll('[aria-label="Attachments"] [aria-label="Attached file"]').length === ${["picker", "paste", "drop"].indexOf(method) + 1}`), 'PDF preview');
    }
    assert.equal(await evaluate(`document.querySelector('[data-slot="chat-composer"] img') === null`), true);
    await click('[aria-label="Send message"]');
    await until(() => evaluate(`!document.querySelector('[data-slot="chat-composer"] [aria-label="Attachments"]')`), 'PDF draft sent');
    const sent = requests.filter(r => r.path === '/agents/run/reply' && r.source === undefined).at(-1);
    assert.equal(sent.body.attachments.length, 3);
  });

  await check("image picker, paste and drop retain drafts per machine and retry failed sends", async () => {
    async function attach(method, name) {
      await evaluate(`(() => {
        const bytes=Uint8Array.from(atob('${pngBase64}'), c=>c.charCodeAt(0));
        const file=new File([bytes], '${name}', {type:'image/png'});
        const dt=new DataTransfer();dt.items.add(file);
        const form=document.querySelector('[data-slot="chat-composer"]');
        if ('${method}'==='picker') {const input=form.querySelector('input[type=file]');input.files=dt.files;input.dispatchEvent(new Event('change',{bubbles:true}));}
        else if ('${method}'==='paste') form.dispatchEvent(new ClipboardEvent('paste',{clipboardData:dt,bubbles:true,cancelable:true}));
        else form.dispatchEvent(new DragEvent('drop',{dataTransfer:dt,bubbles:true,cancelable:true}));
      })()`);
      await until(() => evaluate(`!!document.querySelector('img[alt="${name}"]')?.naturalWidth`), "image preview loaded");
    }
    await open(main);
    await attach("picker", "local-shot.png");
    await open(remoteMain);
    assert.equal(await evaluate(`!!document.querySelector('[data-slot="chat-composer"] [aria-label="Attachments"]')`), false);
    await attach("paste", "remote-paste.png");
    await attach("drop", "remote-drop.png");
    await click('[aria-label="Remove remote-drop.png"]');
    await open(main);
    assert.ok(await evaluate(`!!document.querySelector('img[alt="local-shot.png"]')`));
    await click('[aria-label="Remove local-shot.png"]');
    await open(remoteMain);
    failImageReply = true;
    await click('[data-slot="chat-composer"] [aria-label="Send message"]');
    await until(() => evaluate(`document.querySelector('[data-slot="chat-composer"]').textContent.includes("Message could not be sent")`), "failed image send reported");
    assert.ok(await evaluate(`!!document.querySelector('[aria-label="Attachments"] img')`), "failed send retains image");
    await click('[data-slot="chat-composer"] [aria-label="Send message"]');
    await until(() => evaluate(`!document.querySelector('[data-slot="chat-composer"] [aria-label="Attachments"]')`), "accepted image clears draft");
    assert.ok(await evaluate(`!!document.querySelector('[aria-label="Attachments"] img')?.naturalWidth`), "sent image renders in transcript");
    const sent=requests.filter(r=>r.path==='/agents/run/reply'&&r.source===remote).at(-1);
    assert.equal(sent.body.message, "");
    assert.equal(sent.body.attachments.length, 1);
    await screenshot("image-attachment");
    await open(main);
  });

  await check("identical agent and run ids keep source-specific transcripts and drafts", async () => {
    await type(composer, "Local unsent draft");
    await open(remoteMain); assert.equal(await draft(), "");
    assert.equal(await contains("local fixture answer shared-run"), false);
    await type(composer, "Remote unsent draft");
    await open(main); assert.equal(await draft(), "Local unsent draft");
    await open(remoteMain); assert.equal(await draft(), "Remote unsent draft");
  });

  await check("folded activity caches and requests are scoped to the selected machine", async () => {
    for (const ref of [main, remoteMain]) {
      await open(ref);
      await click(`${chat} [data-slot="chat-tool-call-trigger"]`);
      await until(() => evaluate(`${q(chat)}.textContent.includes(${JSON.stringify(`${localName(ref.source)}-fixture`)})`), "source-specific folded tool");
      assert.ok(requests.some((request) => request.path === "/agents/run/steps" && request.source === ref.source));
      assert.equal(await evaluate(`${q(chat)}.textContent.includes(${JSON.stringify(`${ref.source ? "local" : remote}-fixture`)})`), false);
    }
  });

  await check("late reads cannot replace a different source with the same chat identity", async () => {
    await open(main); holdPeek = idOf(main);
    await click(`${chat} [aria-label="Refresh chat"]`);
    await until(() => heldPeek !== null, "local read held");
    await open(remoteMain);
    await heldPeek.release(); heldPeek = null;
    await pause(100);
    assert.equal(await activeIs(remoteMain), true);
    assert.equal(await contains("local fixture answer shared-run"), false);
  });

  await check("a delayed local reply cannot clear the remote draft or overwrite its transcript", async () => {
    await open(main); await type(composer, "Local held reply"); holdReply = idOf(main);
    await key("Enter"); await until(() => heldReply !== null, "local reply held");
    await open(remoteMain); await type(composer, "Remote draft during local reply");
    await heldReply.release(); heldReply = null;
    await pause(100);
    assert.equal(await draft(), "Remote draft during local reply");
    assert.equal(await contains("local reply accepted"), false);
    await open(main); await until(async () => await draft() === "", "accepted local draft cleared");
  });

  await check("server filters match agent stars and launcher identity while preserving the active chat", async () => {
    await filter("All sessions");
    const agentStarted = { ...main, runId: "agent-started" };
    await open(agentStarted);
    await filter("Only started by me");
    await until(() => evaluate(`!!${q(row(agentStarted))}&&!${q(row({ ...remoteMain, runId: "other-human" }))}`), "active agent-started chat survives me filter");
    await filter("Only started by any user");
    await until(() => evaluate(`!!${q(row({ ...main, runId: "other-human" }))}`), "other user chat visible");
    await filter("Only starred");
    await until(() => evaluate(`!${q(row({ agent: "layout-reviewer", runId: "run-starred-only" }))}`), "run star does not substitute for agent star");
    assert.equal(await evaluate(`!!${q(row(question))}`), true, "pending question bypasses starred filter");
    await type('[aria-label="Search conversations"]', "shared chat");
    await until(() => evaluate(`document.querySelectorAll('[data-slot="app-conversation-link"]').length===2`), "search composes with filter across sources");
    await type('[aria-label="Search conversations"]', "");
    await filter("All sessions");
  });

  await check("older sources that ignore server filters still respect launcher and agent-star narrowing", async () => {
    ignoreRemoteFilters = true;
    await filter("Only started by me");
    await until(() => evaluate(`${q(row(remoteMain))}?.textContent.includes('legacy server')`), "unfiltered legacy response has reached the UI");
    assert.equal(await evaluate(`!!${q(row({ ...remoteMain, runId: "other-human" }))}`), false);
    assert.equal(await evaluate(`!!${q(row({ ...remoteMain, runId: "agent-started" }))}`), false);
    assert.equal(await evaluate(`!!${q(row(question))}`), true);
    await filter("Only starred");
    assert.equal(await evaluate(`!!${q(row({ agent: "layout-reviewer", runId: "run-starred-only", source: remote }))}`), false);
    assert.equal(await evaluate(`!!${q(row(remoteMain))}`), true);
    ignoreRemoteFilters = false;
    await filter("All sessions");
  });

  await check("remote replies, child chats, questions and stop requests retain their node", async () => {
    await open(remoteMain); await type(composer, "Remote sent reply"); await key("Enter");
    await until(() => contains("laptop-b reply accepted"), "remote reply accepted");
    assert.equal(requests.filter((request) => request.path === "/agents/run/reply").at(-1).source, remote);
    const childLink = `${chat} [data-slot="chat-tool-call-link"]`;
    const childHref = new URL(await evaluate(`${q(childLink)}.href`));
    assert.equal(childHref.searchParams.get("node"), remote);
    await click(childLink); await until(() => activeIs(child), "remote child route");
    await until(() => contains("laptop-b fixture answer child-run"), "remote child transcript");
    const adiHref = new URL(await evaluate(`${q('[aria-label="Open in ADI"]')}.href`));
    assert.equal(adiHref.origin, apiBase === "/api" ? origin : "http://app.adi");
    assert.ok(adiHref.href.includes(remote), "Open in ADI carries the node");
    await open(question);
    await type('[data-slot="app-chat-answer-input"]', "Use the remote answer");
    await click('[data-slot="app-chat-answer-submit"]');
    await until(() => contains("remote question answered"), "remote question settled");
    assert.equal(requests.filter((request) => request.path === "/agents/run/answer").at(-1).source, remote);
    snapshots.get(idOf(remoteMain)).running = true;
    await open(remoteMain); await click(`${chat} [aria-label="Refresh chat"]`);
    await until(() => evaluate(`!!${q(`${chat} [aria-label="Stop response"]`)}`), "remote Stop appears");
    await click(`${chat} [aria-label="Stop response"]`);
    await until(() => requests.some((request) => request.path === "/agents/run/stop" && request.source === remote), "remote Stop sent");
  });

  await check("source selection persists, locked nodes stay disabled and remote new chats keep their source", async () => {
    await click('[aria-label="Conversation sources"]');
    assert.equal(await evaluate(`[...document.querySelectorAll('[role="menuitemcheckbox"]')].find(node=>node.textContent.trim()==='locked-node').disabled`), true);
    await option("menuitemcheckbox", "This machine");
    await key("Escape");
    await until(() => evaluate(`!${q(row(main))}&&!!${q(row(remoteMain))}`), "only remote source remains");
    assert.equal(await evaluate(`${q(row(remoteMain))}.closest('[data-slot="app-conversation-group"]').getAttribute('aria-label')`), remote, "machine grouping retains its heading with one source");
    assert.deepEqual(await evaluate("JSON.parse(localStorage.getItem('adi-session-sources'))"), { local: false, nodes: [remote] });
    await click('[aria-label="New conversation"]');
    await until(() => evaluate(`!new URLSearchParams(location.search).has('run')`), "new remote chat route");
    // History changes before React necessarily mounts the new conversation-keyed composer.
    await until(() => evaluate(`${q(`${chat} [data-slot="app-chat-agent"]`)}!==null&&${q(composer)}?.disabled===false&&${q(`${chat} [data-slot="chat-thread"]`)}===null`), "remote new-chat form is ready");
    await type(composer, "Start on the remote machine"); await key("Enter");
    await until(() => evaluate(`new URLSearchParams(location.search).get('run')?.startsWith('new-source-')`), "created remote chat route");
    assert.equal(await evaluate("new URLSearchParams(location.search).get('node')"), remote);
    assert.equal(requests.filter((request) => request.path === "/agents/run").at(-1).source, remote);
    await reloadWhen(`!!${q(row(remoteMain))}`, "persisted remote source reloads");
    assert.equal(await evaluate(`${q(row(remoteMain))}.closest('[data-slot="app-conversation-group"]').getAttribute('aria-label')`), remote, "single-machine heading survives reload");
    assert.equal(await evaluate(`!!${q(row(main))}`), false);
    await click('[aria-label="Filter conversations"]');
    assert.equal(await evaluate(`[...document.querySelectorAll('[role="menuitemradio"]')].find(node=>node.textContent.trim()==='Only started by me').getAttribute('aria-checked')`), "true", "filter resets on reload");
    await key("Escape");
  });

  await check("renaming a paired-machine chat preserves its owning node", async () => {
    await open(remoteMain);
    await click('[aria-label="Rename chat"]');
    await type('[aria-label="Chat name"]', "Remote review renamed");
    await key("Enter");
    await until(()=>evaluate(`document.querySelector('[data-slot="app-chat"] h1')?.textContent==='Remote review renamed'`),"remote header renamed");
    const rename=requests.filter(request=>request.path==='/agents/run/rename').at(-1);
    assert.equal(rename.source,remote);
    assert.deepEqual(rename.body,{name:remoteMain.agent,run_id:remoteMain.runId,title:"Remote review renamed"});
    await reloadWhen(`document.querySelector('[data-slot="app-chat"] h1')?.textContent==='Remote review renamed'`, "renamed chat survives reload");
  });

  await check("project grouping uses bounded server groups and persists its selection", async () => {
    await group("Project");
    await until(() => requests.some((request) => request.source === remote && new URLSearchParams(request.query).get("group") === "project"), "project index requested");
    await until(() => evaluate(`${q('[data-slot="app-chat-sidebar"]')}.textContent.includes('Design project')`), "project groups render");
    assert.equal(await evaluate("JSON.parse(localStorage.getItem('adi-session-group'))"), "project");
    const projectMore = '[data-slot="app-conversation-group"][aria-label="Design project"] [data-slot="app-conversation-group-more"]';
    await click(projectMore);
    await until(() => requests.some((request) => request.source === remote && new URLSearchParams(request.query).get("group_more") === "design:3"), "project pagination remains scoped to the remote source");
    await settleIndex();
    assert.equal(await evaluate(`!!${q(row({ ...remoteMain, runId: "project-2" }))}`), true);
    await screenshot("desktop-projects");
    await reloadWhen(`${q('[data-slot="app-chat-sidebar"]')}?.textContent.includes('Design project')`, "group persists on reload");
  });

  await check("source and filter menus follow the global theme and stay keyboard dismissible within desktop and mobile viewports", async () => {
    for (const mode of ["light", "dark", "mobile"]) {
      const mobile = mode === "mobile";
      const dark = mode === "dark";
      // The theme toggle lives in the desktop topbar; phone chat screens use their own headers.
      await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
      const toggle = '[data-slot="theme-toggle"]';
      if (await evaluate(`${q(toggle)}.getAttribute('aria-checked')`) !== String(dark)) await click(toggle);
      await until(() => evaluate(`document.documentElement.classList.contains('dark')===${dark}`), "global menu theme applies");
      if (mobile) {
        await send("Emulation.setDeviceMetricsOverride", { width: 390, height: 900, deviceScaleFactor: 1, mobile });
        await click('[aria-label="Back to chats"]');
        await until(() => evaluate(`!!document.querySelector('[data-mobile-screen="chats"] [data-slot="app-chat-sidebar"]')`), "mobile inbox shows the conversation list");
      }
      for (const label of ["Conversation sources", "Filter conversations"]) {
        await click(`[aria-label="${label}"]`);
        const menu = await evaluate(`(() => {
          const node=document.querySelector('[role="menu"]');
          const r=node.getBoundingClientRect();
          const style=getComputedStyle(node);
          const tokens=getComputedStyle(document.documentElement);
          const context=document.createElement('canvas').getContext('2d', {willReadFrequently:true});
          const rgba=color=>{context.clearRect(0,0,1,1);context.fillStyle=color;context.fillRect(0,0,1,1);return [...context.getImageData(0,0,1,1).data]};
          return {left:r.left,right:r.right,bottom:r.bottom,portaled:!document.querySelector('[data-slot="app-layout-surface"]').contains(node),background:rgba(style.backgroundColor),foreground:rgba(style.color),backgroundToken:rgba(tokens.getPropertyValue('--popover')),foregroundToken:rgba(tokens.getPropertyValue('--popover-foreground'))};
        })()`);
        assert.ok(menu.left >= 0 && menu.right <= (mobile ? 391 : 1281) && menu.bottom <= 901, `${mode} ${label} menu fits viewport`);
        assert.equal(menu.portaled, true, `${mode} ${label} is tested outside the chat surface`);
        assert.deepEqual(menu.background, menu.backgroundToken, `${mode} ${label} popup background follows the global theme`);
        assert.deepEqual(menu.foreground, menu.foregroundToken, `${mode} ${label} popup text follows the global theme`);
        assert.ok(menu.background.slice(0,3).every(value => dark ? value < 60 : value > 240), `${mode} ${label} popup has the selected light or dark surface`);
        assert.ok(menu.foreground.slice(0,3).every(value => dark ? value > 230 : value < 50), `${mode} ${label} popup text contrasts with its surface`);
        assert.equal(await evaluate("document.querySelector('[role=menu]').contains(document.activeElement)"), true);
        await screenshot(`${mode}-${label === "Conversation sources" ? "sources" : "filters"}`);
        await key("End");
        assert.equal(await evaluate("document.querySelector('[role=menu]').contains(document.activeElement)&&!document.activeElement.disabled"), true);
        await key("Escape");
        assert.equal(await evaluate("!!document.querySelector('[role=menu]')"), false);
        assert.equal(await evaluate("document.activeElement.getAttribute('aria-label')"), label);
      }
      assert.equal(await evaluate("document.documentElement.scrollWidth<=innerWidth"), true, `${mode} menus do not widen the page`);
    }
    await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
  });

  await check("invalid source deep links cannot fall back to reading or replying to a local chat", async () => {
    const before = requests.filter((request) => request.path === "/agents/run/peek").length;
    for (const node of ["../escape", ""]) {
      const previous = await evaluate("performance.timeOrigin");
      await send("Page.navigate", { url: `${conversationUrl(main)}&node=${encodeURIComponent(node)}` });
      await until(() => evaluate(`performance.timeOrigin!==${previous}&&${q(chat)}?.textContent.includes('This chat source is invalid.')`), "invalid source is reported", 15000);
      assert.equal(await evaluate(`${q(composer)}.disabled`), true);
      assert.equal(requests.filter((request) => request.path === "/agents/run/peek").length, before);
    }
  });

  assert.deepEqual(mockErrors, [], "all API requests matched local fixtures");
  assert.deepEqual(errors, [], "no browser exceptions or console errors");
  console.log(`\n${checks.length} source/filter browser scenarios passed; every app API request intercepted.`);
} catch (error) {
  console.error(`Source/filter browser check failed after ${checks.length} scenarios.`);
  if (errors.length) console.error(`Browser errors: ${errors.join("\n")}`);
  if (mockErrors.length) console.error(`Fixture errors: ${mockErrors.join("\n")}`);
  if (evaluate && socket?.readyState === WebSocket.OPEN) {
    try {
      console.error(JSON.stringify(await evaluate("({url:location.pathname+location.search,viteError:!!document.querySelector('vite-error-overlay'),menus:[...document.querySelectorAll('[role=menu]')].map(node=>node.getAttribute('aria-label')),rows:document.querySelectorAll('[data-slot=app-conversation-link]').length})")));
    } catch { /* The browser may already be closed. */ }
  }
  throw error;
} finally {
  for (const held of [heldPeek, heldReply]) { try { await held?.release(); } catch { /* Navigation may have cancelled this fixture. */ } }
  socket?.close(); chrome.kill(); await chrome.exited;
  await rm(profile, { recursive: true, force: true });
}
