/**
 * End-to-end checks of the live React app chat, using isolated Chrome and intercepted API fixtures.
 * Every /api/app/* request is fulfilled locally; this script never forwards a chat mutation.
 * Run: bun frontend/tests/app-chat.browser.mjs
 * Optional: SHOWCASE_URL=http://design-system.adi CHROME_BIN=/path/to/chrome
 * Optional: BROWSER_SCREENSHOTS_DIR=/tmp/chat-screenshots
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

const main = { agent: "design-assistant", runId: "design-main" };
const other = { agent: "design-assistant", runId: "design-other" };
const child = { agent: "layout-reviewer", runId: "review-run" };
const question = { agent: "design-assistant", runId: "question-run" };
const markdown = { agent: "design-assistant", runId: "markdown-run" };
const compact = { agent: "design-assistant", runId: "compact-run" };
const compactText = "A long message should keep a clear gap between the speaker and the opposite edge of the transcript. ".repeat(9);
const markdownCode = `const html = "<section>escaped & preserved</section>";\n  const indented = true;  \nconst longLine = "${"code-width-".repeat(25)}";\n`;
const markdownOutput = "**tool output stays literal**\n<h2>tool markup stays literal</h2>";
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
].join("\n");
const capabilities = { answerable: true, interactive: false, history: true, images: false, tool_steps: true };
const snapshots = new Map();
const titles = new Map();
const requests = [];
const errors = [];
const mockErrors = [];
const checks = [];
let socket;
let send;
let evaluate;
let failNextReply = false;
let holdNextPeek = null;
let heldPeek = null;
let holdNextReply = false;
let heldReply = null;
let holdNextStart = false;
let heldStart = null;
let created = 0;
let failNextRename = false;
let failTranscription = false;
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
    runs: [...snapshots.values()].filter((snapshot) => snapshot.name === name).map((snapshot, index) => ({
      run_id: snapshot.run_id, caps: capabilities, started_at: 5000 - index * 100,
      last_activity: 5000 - index * 100, message: titles.get(`${name}:${snapshot.run_id}`),
      title: titles.get(`${name}:${snapshot.run_id}`), running: snapshot.running, hidden: false, starred: false, launched_by: "human",
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
      await fulfill(requestId, { agents: ["design-assistant", "layout-reviewer"].map((name) => ({ name, running: false, runnable: true, backend: "process:fixture", caps: capabilities })) });
      return;
    }
    if (request.method === "GET" && path === "/api/app/agents/runs/all") {
      assert.equal(url.searchParams.get("hidden"), "false");
      await fulfill(requestId, allRuns());
      return;
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
      snapshot.turns.push(next);
      if (!snapshot.running) snapshot.turns.push(assistant("Mock reply accepted", snapshot.turns.length));
      snapshot.total_turns = snapshot.turns.length;
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
    if (request.method === "POST" && path === "/api/app/agents/run/answer" && snapshot) {
      assert.equal(body.ask, snapshot.pending_question?.id, "answer identifies the current ask");
      delete snapshot.pending_question;
      snapshot.turns.push(user("Mock decision recorded", snapshot.turns.length));
      snapshot.total_turns = snapshot.turns.length;
      await fulfill(requestId, copy(snapshot));
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
    await click(listLink(ref));
    await until(() => activeIs(ref), "selected conversation route");
    if (text) await until(() => contains(text), "selected conversation transcript");
  }
  async function check(name, fn) {
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
    assert.equal(await contains("Accepted fixture reply"), true);
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

  await check("an accepted reply clears only its own draft after navigating to another chat", async () => {
    holdNextReply = true;
    await type(composer, "Accepted after switching chats");
    await key("Enter");
    await until(() => heldReply !== null, "reply acceptance is held");
    await open(other, "Mock other answer");
    await type(composer, "Other chat draft stays");
    await heldReply.release();
    heldReply = null;
    await pause(100);
    assert.equal(await activeIs(other), true);
    assert.equal(await draft(), "Other chat draft stays");
    await open(main, "Mock main answer");
    assert.equal(await draft(), "", "accepted draft does not return after its composer unmounted");
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

  await check("child-agent activity has a real conversation link and opens that chat", async () => {
    const link = `${chat} [data-kind="agent"] [data-slot="chat-tool-call-link"]`;
    assert.equal(await evaluate(`${q(link)}.getAttribute('href')`), `/interface?agent=${child.agent}&run=${child.runId}`);
    await click(link);
    await until(() => activeIs(child), "child conversation route");
    await until(() => contains("Mock child answer"), "child conversation transcript");
  });

  await check("new chat starts the selected agent and opens the run returned by the server", async () => {
    await click('[data-slot="app-chat-sidebar"] [aria-label="New conversation"]');
    await until(() => evaluate(`new URLSearchParams(location.search).get('agent')==='layout-reviewer' && !new URLSearchParams(location.search).has('run')`), "new chat keeps the selected agent");
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
    if (process.env.BROWSER_SCREENSHOTS_DIR) {
      await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
      const capture = await send("Page.captureScreenshot", {format:"png"});
      await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR,"queued-message.png"),Buffer.from(capture.data,"base64"));
    }
    assert.equal(await evaluate(`${q('[data-slot="chat-thread"]')}?.querySelector('[data-queued="true"]') ?? null`), null, "queued message is outside the transcript");
    await click('[data-slot="chat-queued-message"] summary');
    assert.equal(await evaluate(`${q('[data-slot="chat-queued-message"]')}.open`), true);
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
    snapshot.awaits = [{ id: "unknown", note: " ", summary: "", check: "never-show-this-command" }];
    await click('[aria-label="Refresh chat"]');
    await until(() => evaluate(`${q(status)}?.textContent.includes('Waiting on 1 task')`), "count updates");
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

  await check("chat titles rename, retain edits on error, cancel and restore the default", async () => {
    await open(main, "Mock main answer");
    await click('[aria-label="Rename chat"]');
    await type('[aria-label="Chat name"]', "My design review");
    failNextRename = true;
    await key("Enter");
    await until(() => evaluate(`document.querySelector('[data-slot="chat-rename"]')?.textContent.includes('Fixture rename refused')`), "rename error retains editor");
    assert.equal(await evaluate(`document.querySelector('[aria-label="Chat name"]').value`), "My design review");
    await click('[aria-label="Save chat name"]');
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

  await check("mobile navigation, keyboard dismissal and both themes keep chat within the viewport", async () => {
    await open(main, "Mock main answer");
    for (const mode of ["light", "dark", "mobile"]) {
      const mobile = mode === "mobile";
      await setTheme(mode === "dark");
      await send("Emulation.setDeviceMetricsOverride", { width: mobile ? 390 : 1280, height: 900, deviceScaleFactor: 1, mobile });
      if (mobile) {
        const toggle = '[data-slot="app-chat-sidebar"] button[aria-controls]';
        assert.equal(await evaluate(`${q(toggle)}.getAttribute('aria-expanded')`), "false");
        await click(toggle);
        assert.equal(await evaluate(`${q(toggle)}.getAttribute('aria-expanded')`), "true");
        await key("Escape");
        assert.equal(await evaluate(`${q(toggle)}.getAttribute('aria-expanded')`), "false");
        assert.equal(await evaluate(`document.activeElement===${q(toggle)}`), true);
        await click(toggle);
        await open(other, "Mock other answer");
        assert.equal(await evaluate(`${q(toggle)}.getAttribute('aria-expanded')`), "false");
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
