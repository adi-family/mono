/**
 * Settings interactions against isolated Chrome. Every API request is intercepted and answered
 * from an in-memory machine, so no real trigger or secret is read, saved, fired or deleted.
 * Run: bun frontend/tests/settings.browser.mjs
 * Optional: SHOWCASE_URL (default http://design-system.adi), CHROME_BIN, BROWSER_SCREENSHOTS_DIR.
 * The app build can be checked before it is installed: `bun run build:primary`, serve it with
 * `bun x vite preview --outDir dist-primary --port 4179`, and run this with
 * SHOWCASE_URL=http://127.0.0.1:4179 PREVIEW_BUILD=1 — that is where revealing a value and a
 * returning sign-in are exercised, since the showcase offers neither.
 */
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

const origin = new URL(process.env.SHOWCASE_URL ?? "http://design-system.adi").origin;
const chromePath = process.env.CHROME_BIN ?? (process.platform === "darwin"
  ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : "google-chrome");
const profile = await mkdtemp(resolve(tmpdir(), "adi-settings-smoke-"));
const chrome = Bun.spawn([
  chromePath, "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`,
  "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "about:blank",
], { stdout: "ignore", stderr: "ignore" });

// A bare preview of the build has no web manifest; the ADI app serves its own.
const previewBuild = process.env.PREVIEW_BUILD === "1";
const errors = [];
const blocked = [];
const interceptionErrors = [];
const requests = [];
const checks = [];
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
const q = (selector) => `document.querySelector(${JSON.stringify(selector)})`;
let stage = "startup";
let socket;
let send;
let evaluate;

async function until(fn, description, timeout = 20000) {
  const deadline = Date.now() + timeout;
  let last;
  do {
    try { const result = await fn(); if (result) return result; } catch (error) { last = error; }
    await pause(50);
  } while (Date.now() < deadline);
  throw new Error(`Timed out: ${description}${last ? ` (${last.message})` : ""}`);
}

// ---- the fixture machines ------------------------------------------------------------------
const now = Math.floor(Date.now() / 1000);
const vocabulary = {
  kinds: [
    { id: "webhook", label: "Webhook", hint: "runs on a call to /api/hooks/<name>" },
    { id: "background", label: "Background", hint: "runs continuously; auto-restarts" },
    { id: "event", label: "Event", hint: "runs when a matching platform event fires" },
  ],
  runtimes: [{ id: "sh", label: "Shell", hint: "run with sh -c" }, { id: "ts", label: "TypeScript", hint: "run with bun" }],
  presets: [
    { id: "telegram-notify", label: "Telegram notification", description: "Send the payload to a chat", kind: "webhook", runtime: "sh", code: "curl -s \"https://api.telegram.org/bot$TOKEN/sendMessage\"", fields: [{ key: "token_env", label: "Token variable", hint: "the name of the secret holding the bot token", default: "TELEGRAM_TOKEN" }, { key: "chat_id", label: "Chat id", hint: "" }], events: [] },
    { id: "interval", label: "Every N seconds", description: "Run a command on a fixed interval", kind: "background", runtime: "sh", code: "while true; do echo tick; sleep \"$ADI_INTERVAL_SECS\"; done", fields: [{ key: "interval_secs", label: "Interval (seconds)", hint: "", default: "60" }], events: [] },
    { id: "event", label: "On a platform event", description: "React to a platform event", kind: "event", runtime: "ts", code: "console.log(process.env.ADI_EVENT)", fields: [], events: ["adi.tasks.*"] },
  ],
  event_types: [
    { name: "adi.tasks.created", summary: "A task was created", schema: {}, example: { id: "t1", project: "one" } },
    { name: "adi.agents.run.finished", summary: "An agent run finished", schema: {}, example: null },
  ],
};
const trigger = (fields) => ({ runtime: "sh", code: "", preset: null, description: "", enabled: true, project: null, extra: {}, events: [], trigger_on: [], created_at: now - 9000, updated_at: now - 9000, last_fired_at: null, running: false, pid: null, uptime_secs: null, restarts: 0, ...fields });
const machinesState = {
  "": [
    trigger({ name: "deploy-hook", kind: "webhook", description: "Ship the site when the repository is pushed.", code: "echo deploy", extra: { secret: "s3cret" }, last_fired_at: now - 7200 }),
    trigger({ name: "rewards-watch", kind: "background", description: "Poll mainnet every six hours.", code: "while true; do sleep 21600; done", project: "project-one", extra: { interval_secs: "21600" }, running: true, pid: 4242, uptime_secs: 97498, last_fired_at: now - 30 }),
    trigger({ name: "task-events", kind: "event", runtime: "ts", description: "Forward task events.", code: "console.log(1)", enabled: false, project: "project-two", events: ["adi.tasks.*"], trigger_on: ["project-two"] }),
  ],
  studio: [trigger({ name: "studio-loop", kind: "background", description: "Runs on the paired machine.", code: "sleep 1", running: true, pid: 7, uptime_secs: 61 })],
};
const logs = { "deploy-hook": "deploying…\ndone in 4s\n" };
const stateOf = (source) => ({ triggers: [...machinesState[source ?? ""]].sort((a, b) => a.name.localeCompare(b.name)), ...vocabulary });
const fleet = { nodes: [{ node: "studio", locked: false }, { node: "vault", locked: true }] };
const mutations = [];
let failNextSave = null;
let logReads = 0;

// ---- the fixture machines' secrets ---------------------------------------------------------
// Metadata is what the list returns; the values are only ever answered by /secrets/reveal.
const secret = (fields) => ({ project: null, description: null, created_at: now - 86400 * 40, updated_at: now - 86400 * 3, oauth: null, ...fields });
const machineSecrets = {
  "": [
    secret({ name: "OPENAI_API_KEY", description: "Model access for every agent." }),
    secret({ name: "GMAIL_TOKEN", description: "Inbox, read only.", oauth: { provider: "google", obtained_at: now - 4000, expires_at: now + 1500, scope: "https://www.googleapis.com/auth/gmail.readonly email", has_refresh: true } }),
    secret({ name: "API_KEY" }),
    secret({ name: "API_KEY", project: "project-two", description: "Billing sandbox key." }),
    secret({ name: "DEPLOY_KEY", project: "project-one", description: "Read-only deploy key.", updated_at: now - 7200 }),
  ],
  studio: [secret({ name: "STUDIO_TOKEN", description: "Lives on the paired machine." })],
};
const secretValues = new Map([
  ["||OPENAI_API_KEY", "sk-fixture-openai-0001"], ["||GMAIL_TOKEN", "ya29.fixture-access-token"], ["||API_KEY", "global-api-key-value"],
  ["|project-two|API_KEY", "project-two-api-key-value"], ["|project-one|DEPLOY_KEY", "-----BEGIN KEY-----\nline-two\n-----END KEY-----"],
  ["studio||STUDIO_TOKEN", "studio-token-value"],
]);
const secretsOf = (source) => ({ secrets: machineSecrets[source ?? ""].map((item) => ({ ...item })) });
const reveals = [];
let failNextSecret = null;

/** Answers a /api/secrets request from the fixture; `null` when the path is not a secrets one. */
async function secretsAnswer(request, path, source) {
  const list = machineSecrets[source ?? ""];
  if (request.method === "GET" && path === "/api/secrets") return list ? { status: 200, data: secretsOf(source) } : { status: 502, data: { error: "node not paired" } };
  if (request.method !== "POST" || !path.startsWith("/api/secrets/")) return null;
  const body = JSON.parse(request.postData ?? "null");
  const id = `${source ?? ""}|${body.project ?? ""}|${body.name}`;
  const index = list.findIndex((item) => item.name === body.name && (item.project ?? null) === (body.project ?? null));
  if (path === "/api/secrets/reveal") {
    reveals.push({ name: body.name, project: body.project ?? null, ...(source ? { source } : {}) });
    return index < 0 ? { status: 404, data: { ok: false, error: `no such secret: ${body.name}` } } : { status: 200, data: { project: body.project ?? null, name: body.name, value: secretValues.get(id) } };
  }
  mutations.push({ path, body, ...(source ? { source } : {}) });
  await pause(150);
  if (failNextSecret) { const error = failNextSecret; failNextSecret = null; return { status: 500, data: { ok: false, error } }; }
  if (path === "/api/secrets/set" || path === "/api/secrets/set-oauth") {
    // Stamped when the request arrives, as the machine does: the suite's own start time drifts.
    const arrived = Math.floor(Date.now() / 1000);
    const oauth = path.endsWith("set-oauth") ? { provider: body.provider, obtained_at: arrived, expires_at: body.expires_in ? arrived + body.expires_in : null, scope: body.scope ?? null, has_refresh: !!body.refresh_token } : null;
    const saved = secret({ ...(index >= 0 ? { created_at: list[index].created_at } : { created_at: now }), name: body.name, project: body.project ?? null, description: body.description ?? null, updated_at: now, oauth });
    secretValues.set(id, oauth ? body.access_token : body.value);
    if (index >= 0) list[index] = saved; else list.push(saved);
    return { status: 200, data: secretsOf(source) };
  }
  if (index < 0) return { status: 404, data: { ok: false, error: `no such secret: ${body.name}` } };
  if (path === "/api/secrets/remove") { list.splice(index, 1); secretValues.delete(id); return { status: 200, data: secretsOf(source) }; }
  if (path === "/api/secrets/refresh") { const arrived = Math.floor(Date.now() / 1000); list[index] = { ...list[index], updated_at: arrived, oauth: { ...list[index].oauth, obtained_at: arrived, expires_at: arrived + 3600 } }; secretValues.set(id, "ya29.refreshed-access-token"); return { status: 200, data: secretsOf(source) }; }
  return null;
}

async function intercept({ requestId, request }) {
  const url = new URL(request.url);
  let path = url.pathname.replace(/^\/api\/app(?=\/)/, "/api");
  // A paired machine's API is addressed through the node relay.
  const relayed = /^\/api\/node\/([a-z0-9-]+)\/api(\/.*)$/.exec(path);
  const source = relayed?.[1];
  if (relayed) path = `/api${relayed[2]}`;
  const tag = source ? { source } : {};
  requests.push({ stage, method: request.method, path, ...tag, ...(url.pathname.startsWith("/api/app/") ? { relay: true } : {}) });
  const list = machinesState[source ?? ""];
  let data = {};
  let status = 200;
  const secretsReply = await secretsAnswer(request, path, source);
  if (secretsReply) { status = secretsReply.status; data = secretsReply.data; }
  else if (request.method === "GET" && path === "/api/fleet/nodes") data = fleet;
  else if (request.method === "GET" && path === "/api/triggers") {
    if (!list) { status = 502; data = { error: "node not paired" }; } else data = stateOf(source);
  } else if (request.method === "GET" && path === "/api/triggers/relay-key") {
    // This machine's app knows webhook relays; the paired machine's predates them.
    if (source) { status = 404; data = { ok: false, error: "not found" }; } else data = { public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixtureRelayKey node@this-machine" };
  } else if (request.method === "GET" && path === "/api/projects") data = { projects: [{ id: "project-one", name: "One" }, { id: "project-two", name: "Two" }, { id: "old", name: "Archived", archived_at: 1 }] };
  else if (request.method === "POST" && path === "/api/triggers/log") {
    const { name } = JSON.parse(request.postData ?? "null");
    logReads += 1;
    const found = list?.find((item) => item.name === name);
    if (!found) { status = 404; data = { ok: false, error: `no such trigger: ${name}` }; }
    else data = { name, fired: logs[name] !== undefined, output: logs[name] === undefined ? "" : `${logs[name]}read ${logReads}\n`, fired_at: found.last_fired_at };
  } else if (request.method === "POST" && path.startsWith("/api/triggers/")) {
    const body = JSON.parse(request.postData ?? "null");
    mutations.push({ path, body, ...tag });
    await pause(150);
    const index = list.findIndex((item) => item.name === body.name);
    if (path.endsWith("/save")) {
      if (failNextSave) { status = 400; data = { ok: false, error: failNextSave }; failNextSave = null; }
      else {
        const saved = trigger({ ...(index >= 0 ? list[index] : { created_at: now, last_fired_at: null }), ...body, updated_at: now });
        // The supervisor runs exactly the enabled background triggers.
        saved.running = saved.kind === "background" && saved.enabled;
        saved.pid = saved.running ? 5000 + list.length : null;
        saved.uptime_secs = saved.running ? 1 : null;
        if (index >= 0) list[index] = saved; else list.push(saved);
        data = stateOf(source);
      }
    } else if (index < 0) { status = 404; data = { ok: false, error: `no such trigger: ${body.name}` }; }
    else if (path.endsWith("/delete")) { list.splice(index, 1); data = stateOf(source); }
    else if (path.endsWith("/fire")) { list[index].last_fired_at = now; logs[body.name] = "fired by hand\n"; data = { message: `Fired “${body.name}” (pid 9001).`, state: stateOf(source) }; }
    else if (path.endsWith("/restart")) { list[index].pid = 4343; list[index].uptime_secs = 0; data = { message: `Restarting “${body.name}”.`, state: stateOf(source) }; }
    else { blocked.push(`${request.method} ${path}`); status = 403; }
  } else if (request.method !== "GET") { blocked.push(`${request.method} ${path}`); status = 403; }
  await send("Fetch.fulfillRequest", { requestId, responseCode: status, responseHeaders: [{ name: "Content-Type", value: "application/json" }], body: Buffer.from(JSON.stringify(data)).toString("base64") });
}

async function screenshot(name) {
  if (!process.env.BROWSER_SCREENSHOTS_DIR) return;
  await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
  const shot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
  await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, `settings-${name}.png`), Buffer.from(shot.data, "base64"));
}

try {
  const port = await until(async () => Number((await readFile(resolve(profile, "DevToolsActivePort"), "utf8")).split("\n")[0]), "Chrome debugging port");
  const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  socket = new WebSocket(tabs.find((tab) => tab.type === "page").webSocketDebuggerUrl);
  await new Promise((done, reject) => { socket.onopen = done; socket.onerror = reject; });
  let nextId = 1;
  const pending = new Map();
  send = (method, params = {}) => {
    const id = nextId++;
    return new Promise((done, reject) => {
      const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 15000);
      pending.set(id, { done, reject, timer });
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
      message.error ? waiting.reject(new Error(JSON.stringify(message.error))) : waiting.done(message.result);
    }
    if (message.method === "Fetch.requestPaused") void intercept(message.params).catch((error) => {
      // A full document navigation may cancel a read while it is being answered.
      if (!/Invalid InterceptionId|Invalid interceptionId|Invalid requestId|No resource with given identifier|interception.*not found/i.test(error.message)) interceptionErrors.push(error.message);
    });
    let detail;
    if (message.method === "Runtime.exceptionThrown") detail = message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text;
    if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") detail = message.params.args.map((arg) => arg.value ?? arg.description).join(" ");
    if (message.method === "Log.entryAdded" && message.params.entry.level === "error") detail = message.params.entry.text;
    // The checks below provoke these on purpose: refused saves, a machine that is not paired, and
    // a machine whose app predates webhook relays.
    if (detail && !/Failed to load resource.*\b(400|404|500|502)\b/.test(detail) && !(previewBuild && /^Manifest: /.test(detail))) errors.push({ stage, detail });
  };
  evaluate = async (expression) => {
    const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    return result.result.value;
  };
  async function check(name, fn) {
    stage = name;
    await fn();
    // Give asynchronously logged render/fetch failures time to arrive.
    await pause(300);
    assert.deepEqual(blocked, [], "no unexpected API writes");
    assert.deepEqual(interceptionErrors, [], "API interception remains active");
    assert.deepEqual(errors, [], "zero browser exceptions or console errors");
    checks.push(name);
    console.log(`PASS ${name}`);
  }
  async function navigate(path, selector) {
    await send("Page.navigate", { url: `${origin}${path}` });
    await until(() => evaluate(`!!${q(selector)}`), `${path} renders ${selector}`);
  }
  /** Reload the document and wait for the new one: the old DOM still answers until it is replaced. */
  async function reload(selector) {
    await evaluate("window.beforeReload=true");
    await send("Page.reload");
    await until(() => evaluate(`window.beforeReload===undefined && !!${q(selector)}`), `the reloaded page renders ${selector}`);
  }
  async function click(selector) {
    await until(() => evaluate(`!!${q(selector)}`), `${selector} exists`);
    await evaluate(`${q(selector)}.scrollIntoView({block:'center'})`);
    const point = await evaluate(`(() => {const r=${q(selector)}.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
    await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, ...point });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, ...point });
  }
  async function value(selector, text) {
    await evaluate(`(() => {const el=${q(selector)};const proto={SELECT:HTMLSelectElement,TEXTAREA:HTMLTextAreaElement}[el.tagName]??HTMLInputElement;Object.getOwnPropertyDescriptor(proto.prototype,'value').set.call(el,${JSON.stringify(text)});el.dispatchEvent(new Event('input',{bubbles:true}));el.dispatchEvent(new Event('change',{bubbles:true}));})()`);
  }
  const text = (selector) => evaluate(`${q(selector)}?.textContent ?? null`);
  const field = (name) => `[data-slot="trigger-form"] [name="${name}"]`;
  const row = (name) => `[data-slot="trigger-row"][data-trigger="${name}"]`;
  const rowNames = () => evaluate(`Array.from(document.querySelectorAll('[data-slot="trigger-row"]'),row=>row.dataset.trigger)`);
  const lastMutation = () => mutations.at(-1);
  async function tokenColor(selector, property = "backgroundColor") {
    const color = await until(() => evaluate(`(() => {const el=${q(selector)};if(!el)return null;const color=getComputedStyle(el)[${JSON.stringify(property)}];return color.startsWith('oklch(')?color:null;})()`), `${selector} applies a theme token`);
    assert.match(color, /^oklch\(/);
  }
  const located = (search) => until(() => evaluate(`location.pathname==='/settings/triggers' && location.search===${JSON.stringify(search)}`), `the address becomes /settings/triggers${search}`);

  await Promise.all([send("Runtime.enable"), send("Page.enable"), send("Log.enable"), send("Network.enable")]);
  await send("Network.setBypassServiceWorker", { bypass: true });
  // A headless page is not the focused window; keyboard checks need it to behave as one.
  await send("Emulation.setFocusEmulationEnabled", { enabled: true });
  await send("Fetch.enable", { patterns: [{ urlPattern: "*/api/*", requestStage: "Request" }] });
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });

  await check("Settings opens on Triggers, in the app shell, with the way back to the old settings", async () => {
    await navigate("/settings", row("deploy-hook"));
    await located("");
    assert.equal(await evaluate("document.title"), "Triggers — Settings — ADI");
    assert.equal(await text('[data-slot="app-topbar"] h1'), "Settings");
    const rail = '[data-slot="global-navigation"] a[aria-label="Settings"]';
    assert.equal(await evaluate(`${q(rail)}.getAttribute('href')`), "/settings", "the rail opens the new settings");
    assert.equal(await evaluate(`${q(rail)}.getAttribute('aria-current')`), "page");
    const old = await evaluate(`(() => {const a=${q('[data-slot="settings-old-ui"]')};return {tag:a.tagName,href:a.href,text:a.textContent.trim()};})()`);
    assert.deepEqual([old.tag, old.text], ["A", "Old UI settings"]);
    assert.equal(new URL(old.href).pathname, "/extended/settings", "the old settings stay one click away");
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-slot="settings-nav-item"]'),item=>[item.textContent.trim(),item.getAttribute('aria-current'),item.getAttribute('href')])`), [["Triggers", "page", "/settings/triggers"], ["Secrets", null, "/settings/secrets"]]);
    assert.deepEqual(await rowNames(), ["deploy-hook", "rewards-watch", "task-events"]);
    assert.equal((await text('[data-slot="triggers-count"]')).trim(), "3 triggers · 1 running");
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-slot="trigger-status"]'),badge=>[badge.dataset.status,badge.textContent])`), [["listening", "Listening"], ["running", "Running"], ["disabled", "Disabled"]]);
    assert.match(await text(row("rewards-watch")), /background · sh.*Project project-one.*pid 4242 · up 1d 3h.*Last run (just now|1m ago)/);
    assert.match(await text(row("deploy-hook")), /\/api\/hooks\/deploy-hook.*Last run 2h ago/);
    assert.match(await text(row("task-events")), /adi\.tasks\.\*.*Never run/);
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-slot="trigger-row"] [role="switch"]'),control=>control.getAttribute('aria-checked'))`), ["true", "true", "false"]);
    await tokenColor('[data-slot="settings-triggers"] [data-slot="card"]');
    await tokenColor('[data-slot="trigger-new"]');
    await tokenColor(`${row("rewards-watch")} [data-slot="badge-dot"]`);
    assert.match(await text('button[aria-label="Settings machine"]'), /This machine/, "the topbar names whose settings these are");
    assert.equal(await text('[data-slot="settings-machine"]'), "this machine");
    assert.equal(await evaluate(`${q('[data-slot="app-secondary-navigation"]')}?.hidden ?? true`), true, "no component sidebar beside settings");
    await screenshot("list");
  });

  await check("the row switch re-saves the whole definition with only the flag changed", async () => {
    await click(`${row("task-events")} [role="switch"]`);
    await until(() => evaluate(`${q(`${row("task-events")} [role="switch"]`)}.getAttribute('aria-checked')==='true'`), "the switch shows the saved state");
    assert.deepEqual(lastMutation(), { path: "/api/triggers/save", body: { name: "task-events", kind: "event", runtime: "ts", code: "console.log(1)", preset: null, description: "Forward task events.", enabled: true, project: "project-two", extra: {}, events: ["adi.tasks.*"], trigger_on: ["project-two"] } });
    assert.equal(await text(`${row("task-events")} [data-slot="trigger-status"]`), "Listening");
    await click(`${row("rewards-watch")} [role="switch"]`);
    await until(async () => (await text(`${row("rewards-watch")} [data-slot="trigger-status"]`)) === "Disabled", "a disabled background trigger stops");
    assert.equal(lastMutation().body.enabled, false);
    assert.deepEqual(lastMutation().body.extra, { interval_secs: "21600" });
    assert.equal((await text('[data-slot="triggers-count"]')).trim(), "3 triggers");
    await click(`${row("rewards-watch")} [role="switch"]`);
    await until(async () => (await text(`${row("rewards-watch")} [data-slot="trigger-status"]`)) === "Running", "enabling starts it again");
  });

  await check("a trigger opens in the same page; only a changed definition can be saved", async () => {
    await evaluate("window.settingsMarker=42");
    await click(`${row("deploy-hook")} a[href="/settings/triggers?trigger=deploy-hook"]`);
    await until(() => evaluate(`!!${q(field("name"))}`), "the definition form opens");
    await located("?trigger=deploy-hook");
    assert.equal(await evaluate("window.settingsMarker"), 42, "no document navigation");
    assert.deepEqual(await evaluate(`Object.fromEntries(['name','description','kind','runtime','project','code'].map(name=>[name,document.querySelector('[data-slot="trigger-form"] [name="'+name+'"]').value]))`), { name: "deploy-hook", description: "Ship the site when the repository is pushed.", kind: "webhook", runtime: "sh", project: "", code: "echo deploy" });
    assert.equal(await evaluate(`${q(field("name"))}.readOnly`), true, "an existing trigger keeps its name");
    assert.equal(await evaluate(`${q(field("enabled"))}.getAttribute('aria-checked')`), "true");
    // A webhook's secret is offered, hidden until asked for.
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-slot="trigger-settings"] input'),input=>[input.dataset.setting,input.type,input.value])`), [["secret", "password", "s3cret"]]);
    await click('button[aria-label="Show the secret"]');
    assert.equal(await evaluate(`${q('input[data-setting="secret"]')}.type`), "text");
    assert.equal(await text('[data-slot="settings-trigger"] [data-slot="trigger-status"]'), "Listening");
    assert.equal(await evaluate(`${q('[data-slot="trigger-save"]')}.disabled`), true, "nothing to save yet");
    assert.equal(await evaluate(`!!${q('[data-slot="trigger-fire"]')} && !${q('[data-slot="trigger-restart"]')}`), true, "a trigger that is not running can be fired once");
    // Projects other than archived ones can restrict a webhook.
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-slot="trigger-allowlist"] label'),label=>[label.textContent.trim(),label.querySelector('input').checked])`), [["One", false], ["Two", false]]);
    await value(field("description"), "  Ship the site on push  ");
    await value(field("project"), "project-one");
    await click('[data-slot="trigger-allowlist"] label:nth-child(2) input');
    assert.equal(await evaluate(`${q('[data-slot="trigger-save"]')}.disabled`), false);
    const before = mutations.length;
    await click('[data-slot="trigger-save"]');
    await until(async () => (await text('[data-slot="trigger-notice"]')) === "Saved “deploy-hook”.", "the save is confirmed");
    assert.equal(mutations.length, before + 1, "one save");
    assert.deepEqual(lastMutation(), { path: "/api/triggers/save", body: { name: "deploy-hook", kind: "webhook", runtime: "sh", code: "echo deploy", preset: null, description: "Ship the site on push", enabled: true, project: "project-one", extra: { secret: "s3cret" }, events: [], trigger_on: ["project-two"] } });
    await until(() => evaluate(`${q('[data-slot="trigger-save"]')}.disabled`), "the saved form has nothing left to save");
    await screenshot("definition");
  });

  await check("a refused save stays in the form with the machine's reason", async () => {
    failNextSave = "invalid trigger: the code block is empty";
    await value(field("code"), "echo changed");
    await click('[data-slot="trigger-save"]');
    await until(async () => (await text('[data-slot="trigger-form-error"]')) === "invalid trigger: the code block is empty", "the reason is shown");
    assert.equal(await evaluate(`${q(field("code"))}.value`), "echo changed", "what was typed is kept");
    assert.equal(machinesState[""].find((item) => item.name === "deploy-hook").code, "echo deploy", "nothing was saved");
    await click('[data-slot="trigger-save"]');
    await until(async () => (await text('[data-slot="trigger-notice"]')) === "Saved “deploy-hook”.", "the retry saves");
    assert.equal(await evaluate(`!${q('[data-slot="trigger-form-error"]')}`), true);
  });

  await check("Fire once runs the code block by hand, and the Log view follows the output", async () => {
    await click('[data-slot="trigger-fire"]');
    await until(async () => (await text('[data-slot="trigger-notice"]')) === "Fired “deploy-hook” (pid 9001).", "the fire is reported");
    assert.deepEqual(lastMutation(), { path: "/api/triggers/fire", body: { name: "deploy-hook" } });
    await click('nav[aria-label="Trigger views"] a[href="/settings/triggers?trigger=deploy-hook&view=log"]');
    await until(() => evaluate(`!!${q('[data-slot="trigger-log"] pre')}`), "the log opens");
    await located("?trigger=deploy-hook&view=log");
    assert.equal(await evaluate(`${q('nav[aria-label="Trigger views"] a[aria-current="page"]')}.textContent`), "Log");
    const first = await text('[data-slot="trigger-log"] pre');
    assert.match(first, /^fired by hand\nread \d+\n$/);
    await until(async () => (await text('[data-slot="trigger-log"] pre')) !== first, "the log is re-read while it is open", 8000);
    assert.equal(await evaluate(`!${q('[data-slot="trigger-notice"]')}`), true, "a result message does not follow into another view");
    // A hard reload of a trigger's view keeps the React page and the view.
    await reload('[data-slot="trigger-log"] pre');
    assert.equal(await evaluate(`!!${q('[data-slot="global-navigation"]')}`), true);
    await screenshot("log");
    await navigate("/settings/triggers?trigger=task-events&view=log", '[data-slot="trigger-log-empty"]');
    assert.match(await text('[data-slot="trigger-log-empty"]'), /never run/);
  });

  await check("a running background trigger offers Restart instead of Fire", async () => {
    await navigate("/settings/triggers?trigger=rewards-watch", '[data-slot="trigger-restart"]');
    assert.equal(await evaluate(`!${q('[data-slot="trigger-fire"]')}`), true);
    assert.match(await text('[data-slot="settings-trigger"] header'), /pid 4242|pid 500\d/);
    await click('[data-slot="trigger-restart"]');
    await until(async () => (await text('[data-slot="trigger-notice"]')) === "Restarting “rewards-watch”.", "the restart is reported");
    assert.deepEqual(lastMutation(), { path: "/api/triggers/restart", body: { name: "rewards-watch" } });
    // Settings a preset does not declare are still offered, so an old definition loses nothing.
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-slot="trigger-settings"] input'),input=>[input.dataset.setting,input.value])`), [["interval_secs", "21600"]]);
  });

  await check("the code block is an editor: highlighted, numbered, and it indents like one", async () => {
    await navigate("/settings/triggers?trigger=rewards-watch", '[data-slot="code-editor"] textarea');
    const editor = '[data-slot="code-editor"]';
    const input = `${editor} textarea[name="code"]`;
    const state = () => evaluate(`(() => {const el=${q(input)};return {value:el.value,start:el.selectionStart,end:el.selectionEnd};})()`);
    const key = async (name, code, modifiers = 0) => {
      const params = { key: name, code: name, windowsVirtualKeyCode: code, modifiers };
      await send("Input.dispatchKeyEvent", { type: "keyDown", ...params });
      await send("Input.dispatchKeyEvent", { type: "keyUp", ...params });
    };
    // Place the selection the way a click or an arrow key would leave it, then let the page hear about it.
    const select = (start, end = start) => evaluate(`(() => {const el=${q(input)};el.focus();el.setSelectionRange(${start},${end});document.dispatchEvent(new Event('selectionchange'));el.dispatchEvent(new KeyboardEvent('keyup',{bubbles:true,key:'ArrowRight'}));})()`);
    assert.equal(await evaluate(`${q(editor)}.dataset.language`), "sh", "the runtime picks the language");
    assert.equal(await text(`${editor} [data-slot="code-editor-label"]`), "Shell");
    assert.equal(await evaluate(`${q(input)}.value`), "while true; do sleep 21600; done");
    // The paint is the same text, token by token.
    assert.equal(await text(`${editor} [data-slot="code-editor-paint"]`), "while true; do sleep 21600; done");
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('${editor} [data-token]'),span=>span.dataset.token+':'+span.textContent)`), ["kw:while", "punct:;", "kw:do", "num:21600", "punct:;", "kw:done"]);
    const inks = await evaluate(`(() => {const color=s=>getComputedStyle(document.querySelector(s)).color;return {kw:color('${editor} [data-token="kw"]'),num:color('${editor} [data-token="num"]'),plain:color('${editor} [data-slot="code-editor-paint"]'),typed:getComputedStyle(${q(input)}).webkitTextFillColor};})()`);
    assert.match(inks.kw, /^oklch\(/);
    assert.equal(new Set([inks.kw, inks.num, inks.plain]).size, 3, "keywords and numbers have ink of their own");
    assert.match(inks.typed, /rgba\(0, 0, 0, 0\)|transparent/, "the textarea's own glyphs are invisible; the paint shows through");
    // The two layers must agree on everything that places a glyph, or the caret drifts off its text.
    const metrics = await evaluate(`(() => {const m=el=>{const c=getComputedStyle(el);const r=el.getBoundingClientRect();return [c.fontFamily,c.fontSize,c.lineHeight,c.paddingLeft,c.paddingTop,c.tabSize,c.whiteSpace,c.letterSpacing,Math.round(r.left),Math.round(r.top)].join('|');};return [m(${q(input)}),m(document.querySelector('${editor} [data-slot="code-editor-paint"]'))];})()`);
    assert.equal(metrics[0], metrics[1], "paint and textarea share one set of glyph metrics and one origin");
    assert.equal(await text(`${editor} [data-slot="code-editor-gutter"]`), "1");
    await tokenColor(editor, "borderTopColor");

    // Typing is the browser's own; Enter keeps the indentation; Tab and Shift+Tab move whole lines.
    await click(input);
    await value(input, "while true; do\n  sleep 1\ndone");
    await until(async () => (await text(`${editor} [data-slot="code-editor-gutter"]`)) === "1\n2\n3", "a line number per line");
    await select(24);
    await until(async () => (await text(`${editor} [data-slot="code-editor-position"]`)).trim() === "Ln 2, Col 10", "the caret position is shown");
    await key("Enter", 13);
    await send("Input.insertText", { text: "echo $HOME" });
    assert.equal((await state()).value, "while true; do\n  sleep 1\n  echo $HOME\ndone");
    await until(() => evaluate(`Array.from(document.querySelectorAll('${editor} [data-token="func"]'),span=>span.textContent).join()==='$HOME'`), "new text is highlighted as it is typed");
    await key("Tab", 9);
    assert.deepEqual(await state(), { value: "while true; do\n  sleep 1\n  echo $HOME  \ndone", start: 39, end: 39 }, "Tab at a caret inserts one indent");
    await select(17, 30);
    await key("Tab", 9);
    assert.deepEqual(await state(), { value: "while true; do\n    sleep 1\n    echo $HOME  \ndone", start: 19, end: 34 }, "Tab over a selection indents its lines and keeps them selected");
    await key("Tab", 9, 8);
    await key("Tab", 9, 8);
    assert.deepEqual(await state(), { value: "while true; do\nsleep 1\necho $HOME  \ndone", start: 15, end: 26 }, "Shift+Tab outdents them");
    // The browser's undo history holds each of those edits.
    await evaluate("document.execCommand('undo')");
    assert.equal((await state()).value, "while true; do\n  sleep 1\n  echo $HOME  \ndone", "an indent is one undo step");
    // The keyboard is never trapped: Escape, then Tab, moves on.
    await key("Escape", 27);
    await key("Tab", 9);
    assert.equal(await evaluate(`document.activeElement !== ${q(input)}`), true, "Escape then Tab leaves the editor");
    assert.equal((await state()).value, "while true; do\n  sleep 1\n  echo $HOME  \ndone", "leaving changes nothing");

    // A long block scrolls inside the editor, and the paint and the line numbers follow it.
    await value(input, Array.from({ length: 80 }, (_, index) => `echo line-${index + 1} "$X" # ${"wide ".repeat(40)}`).join("\n"));
    await until(async () => (await text(`${editor} [data-slot="code-editor-gutter"]`)).endsWith("\n80"), "eighty line numbers");
    await evaluate(`(() => {const el=${q(input)};el.scrollTop=400;el.scrollLeft=300;el.dispatchEvent(new Event('scroll'));})()`);
    const scrolled = await evaluate(`(() => {const el=${q(input)};return {top:el.scrollTop,left:el.scrollLeft,paint:document.querySelector('${editor} [data-slot="code-editor-paint"]').style.transform,gutter:document.querySelector('${editor} [data-slot="code-editor-gutter"] > div').style.transform};})()`);
    assert.equal(scrolled.paint, `translate(${-scrolled.left}px, ${-scrolled.top}px)`);
    assert.equal(scrolled.gutter, `translateY(${-scrolled.top}px)`);
    assert(scrolled.top > 0 && scrolled.left > 0, "the block scrolls both ways instead of wrapping");
    assert(await evaluate("document.documentElement.scrollWidth<=document.documentElement.clientWidth"), "a wide line never widens the page");
    await screenshot("editor");

    // Another language is another scanner, on the same text.
    await value(input, "// watch\nconst n = await fetch(url);");
    await value(field("runtime"), "ts");
    await until(() => evaluate(`${q(editor)}.dataset.language==='ts'`), "the editor follows the runtime picker");
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('${editor} [data-token]'),span=>span.dataset.token+':'+span.textContent)`), ["comment:// watch", "kw:const", "kw:await", "func:fetch", "punct:(", "punct:);"]);
    assert.equal(await text(`${editor} [data-slot="code-editor-label"]`), "TypeScript");
    await click('[data-slot="trigger-save"]');
    await until(async () => (await text('[data-slot="trigger-notice"]')) === "Saved “rewards-watch”.", "the edited block is saved");
    assert.deepEqual([lastMutation().body.runtime, lastMutation().body.code], ["ts", "// watch\nconst n = await fetch(url);"]);
    await screenshot("editor-ts");
  });

  await check("a webhook's relay has its own fields; the machine's endpoint is shown, never edited, and survives a save", async () => {
    // As the machine leaves it after registering the trigger with its relay.
    Object.assign(machinesState[""].find((item) => item.name === "deploy-hook").extra, { relay: "public-eu", relay_endpoint: "ep_7Hq2", relay_retention: "3600" });
    await navigate("/settings/triggers?trigger=deploy-hook", '[data-slot="trigger-relay"]');
    assert.deepEqual(await evaluate(`[${q(field("relay"))}.value, ${q(field("relay_retention"))}.value]`), ["public-eu", "3600"]);
    assert.deepEqual(await evaluate(`Array.from(${q(field("relay"))}.options,option=>option.textContent)`), ["Direct only — no relay", "Public (EU)", "Public (US)", "Enterprise (EU)", "Enterprise (US)", "Self-hosted (custom URL)"]);
    assert.deepEqual(await evaluate(`(() => {const el=${q('[data-slot="trigger-relay-url"]')};return [el.value, el.readOnly];})()`), ["https://eu.webhooks.withadi.dev/h/ep_7Hq2", true], "the address to give the sender");
    await until(() => evaluate(`!!${q('[data-slot="trigger-relay-key"]')}`), "the machine's relay key loads");
    assert.deepEqual(await evaluate(`(() => {const el=${q('[data-slot="trigger-relay-key"]')};return [el.value, el.readOnly];})()`), ["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixtureRelayKey node@this-machine", true]);
    // The relay's settings are not offered again as free-form settings.
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-slot="trigger-settings"] input'),input=>input.dataset.setting)`), ["secret"]);
    assert.equal(await evaluate(`!${q(field("relay_endpoint"))} && !${q('input[data-setting="relay_endpoint"]')}`), true, "the endpoint is nowhere a text box");
    assert.equal(await evaluate(`${q('[data-slot="trigger-save"]')}.disabled`), true, "an untouched relayed trigger has nothing to save");
    await click('button[aria-label="Copy public url"]');
    await until(async () => /Copied|Couldn’t reach the clipboard/.test(await text('[data-slot="trigger-relay"]')), "the copy is reported");
    await value(field("relay_retention"), "7200");
    await click('[data-slot="trigger-save"]');
    await until(async () => (await text('[data-slot="trigger-notice"]')) === "Saved “deploy-hook”.", "saved");
    assert.deepEqual(lastMutation().body.extra, { relay: "public-eu", relay_endpoint: "ep_7Hq2", relay_retention: "7200", secret: "s3cret" }, "the endpoint rides along untouched");
    // A self-hosted relay is a URL; an enterprise one asks for a licence code.
    await value(field("relay"), "custom");
    await until(() => evaluate(`!!${q(field("relay_url"))}`), "the relay URL is asked for");
    await value(field("relay_url"), "https://relay.example.com");
    await until(async () => (await evaluate(`${q('[data-slot="trigger-relay-url"]')}?.value`)) === "https://relay.example.com/h/ep_7Hq2", "the public address follows the relay");
    await value(field("relay"), "enterprise-us");
    await until(() => evaluate(`!!${q(field("licence_code"))} && !${q(field("relay_url"))}`), "an enterprise relay asks for a licence code");
    // Direct only: no relay fields, and the relay setting is cleared on save.
    await value(field("relay"), "");
    await until(() => evaluate(`!${q(field("relay_retention"))} && !${q('[data-slot="trigger-relay-key"]')}`), "direct only shows no relay fields");
    await click('[data-slot="trigger-save"]');
    await until(async () => lastMutation().body.extra.relay === undefined && lastMutation().body.extra.relay_retention === "7200", "saved without a relay");
    assert.equal(lastMutation().body.extra.relay_endpoint, "ep_7Hq2", "what the machine wrote is still its to remove");
    await screenshot("relay");
    // A trigger that is not a webhook has no inbound endpoint for a relay to front.
    await navigate("/settings/triggers?trigger=rewards-watch", field("code"));
    assert.equal(await evaluate(`!${q('[data-slot="trigger-relay"]')}`), true);
    // A new webhook on this machine is offered a relay, and told where its address will appear.
    await navigate("/settings/triggers?new=1", field("name"));
    await value(field("kind"), "webhook");
    await until(() => evaluate(`!!${q('[data-slot="trigger-relay"]')}`), "a webhook is offered a relay");
    await value(field("relay"), "public-us");
    await until(() => evaluate(`!!${q('[data-slot="trigger-relay-pending"]')}`), "the address is promised after the save");
    assert.equal(await evaluate(`!${q('[data-slot="trigger-relay-url"]')}`), true);
    machinesState[""].find((item) => item.name === "deploy-hook").extra = { secret: "s3cret" };
  });

  await check("a new trigger starts from a preset and is checked before it is sent", async () => {
    await click('a[href="/settings/triggers"]');
    await until(() => evaluate(`!!${q(row("deploy-hook"))}`), "back on the list");
    await click('[data-slot="trigger-new"]');
    await until(() => evaluate(`!!${q(field("name"))}`), "the new trigger form opens");
    await located("?new=1");
    assert.equal(await text('[data-slot="settings-trigger"] h3'), "New trigger");
    assert.equal(await evaluate(`!${q('nav[aria-label="Trigger views"]')} && !${q('[data-slot="trigger-delete"]')}`), true, "a trigger that does not exist yet has no log and nothing to delete");
    const before = mutations.length;
    await click('[data-slot="trigger-save"]');
    await until(async () => (await text('[data-slot="trigger-form-error"]')) === "A trigger name is required.", "a missing name is caught here");
    await click('[data-preset="event"]');
    await until(() => evaluate(`${q(field("kind"))}.value==='event'`), "the preset fills the form");
    assert.deepEqual(await evaluate(`['runtime','code','events','description'].map(name=>document.querySelector('[data-slot="trigger-form"] [name="'+name+'"]').value)`), ["ts", "console.log(process.env.ADI_EVENT)", "adi.tasks.*", "React to a platform event"]);
    assert.equal(await evaluate(`${q('[data-preset="event"]')}.getAttribute('aria-pressed')`), "true");
    // The catalog adds a pattern once.
    await click('[data-slot="trigger-event-catalog"] button:nth-child(2)');
    await click('[data-slot="trigger-event-catalog"] button:nth-child(2)');
    assert.equal(await evaluate(`${q(field("events"))}.value`), "adi.tasks.*\nadi.agents.run.finished");
    await value(field("name"), "bad name");
    await click('[data-slot="trigger-save"]');
    await until(async () => /letters, digits/.test(await text('[data-slot="trigger-form-error"]') ?? ""), "an unusable name is caught here");
    assert.equal(mutations.length, before, "nothing was sent");
    await value(field("name"), "on-task");
    // A setting of the user's own.
    await value('input[aria-label="New setting name"]', "Bad Key");
    await click('[data-slot="trigger-add-setting"]');
    await until(() => evaluate(`!!document.querySelector('[data-slot="trigger-form"] [role="alert"]')`), "an unusable setting name is refused");
    await value('input[aria-label="New setting name"]', "chat_id");
    await click('[data-slot="trigger-add-setting"]');
    await until(() => evaluate(`!!${q('input[data-setting="chat_id"]')}`), "the setting is offered");
    await value('input[data-setting="chat_id"]', " 12345 ");
    await click('[data-slot="trigger-save"]');
    await located("?trigger=on-task");
    assert.deepEqual(lastMutation(), { path: "/api/triggers/save", body: { name: "on-task", kind: "event", runtime: "ts", code: "console.log(process.env.ADI_EVENT)", preset: "event", description: "React to a platform event", enabled: true, project: null, extra: { chat_id: "12345" }, events: ["adi.tasks.*", "adi.agents.run.finished"], trigger_on: [] } });
    assert.equal(mutations.length, before + 1);
    assert.equal(await text('[data-slot="trigger-notice"]'), "Created “on-task”.");
    // The address changes first; the page follows it in a transition.
    await until(() => evaluate(`${q(field("name"))}?.readOnly===true`), "the created trigger is now being edited");
    assert.equal(await text('[data-slot="trigger-notice"]'), "Created “on-task”.", "the confirmation follows the trigger to its own page");
    assert.equal(await evaluate(`!!${q('nav[aria-label="Trigger views"]')}`), true);
    await screenshot("created");
    // A preset link prefills a new trigger without a click.
    await navigate("/settings/triggers?new=1&preset=interval", field("name"));
    assert.deepEqual(await evaluate(`[${q(field("kind"))}.value, ${q('input[data-setting="interval_secs"]')}.value]`), ["background", "60"]);
  });

  await check("deleting asks first, and only the confirmed delete is sent", async () => {
    await navigate("/settings/triggers?trigger=on-task", '[data-slot="trigger-delete"]');
    const before = mutations.length;
    await click('[data-slot="trigger-delete"]');
    await until(() => evaluate(`!!${q('dialog[open][data-slot="confirm-dialog"]')}`), "the question opens");
    assert.match(await text('dialog[open]'), /Delete the trigger “on-task” from this machine\?.*cannot be undone/);
    await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    await until(() => evaluate(`!${q('dialog[open]')}`), "Escape declines");
    assert.equal(mutations.length, before, "declining deletes nothing");
    await click('[data-slot="trigger-delete"]');
    await click('dialog[open] [data-slot="confirm-dialog-confirm"]');
    await until(() => evaluate(`!!${q(row("deploy-hook"))} && !${q(row("on-task"))}`), "the list returns without the trigger");
    await located("");
    assert.deepEqual(lastMutation(), { path: "/api/triggers/delete", body: { name: "on-task" } });
    assert.equal(await text('[data-slot="trigger-notice"]'), "Deleted “on-task”.");
    assert.deepEqual(await rowNames(), ["deploy-hook", "rewards-watch", "task-events"]);
    await navigate("/settings/triggers?trigger=on-task", '[data-slot="trigger-missing"]');
    assert.match(await text('[data-slot="trigger-missing"]'), /No trigger named “on-task”/);
  });

  await check("another machine's settings are read and saved through its relay", async () => {
    await navigate("/settings/triggers", row("deploy-hook"));
    await click('button[aria-label="Settings machine"]');
    const studio = '[data-slot="node-selector-menu"] [role="menuitemradio"]:nth-of-type(2)';
    await until(() => evaluate(`${q(studio)}?.textContent.trim()==='studio'`), "paired machines are offered, one at a time");
    assert.equal(await text('[data-slot="node-selector-menu"] p'), "Settings of");
    await click(studio);
    await until(() => evaluate(`!!${q(row("studio-loop"))} && !${q(row("deploy-hook"))}`), "the paired machine's triggers replace this machine's");
    assert.equal(await text('[data-slot="settings-machine"]'), "studio");
    assert(requests.some((request) => request.path === "/api/triggers" && request.source === "studio"), "the list is read through the node relay");
    await click(`${row("studio-loop")} [role="switch"]`);
    await until(async () => (await text(`${row("studio-loop")} [data-slot="trigger-status"]`)) === "Disabled", "the paired machine's trigger is disabled");
    assert.deepEqual([lastMutation().path, lastMutation().source, lastMutation().body.name, lastMutation().body.enabled], ["/api/triggers/save", "studio", "studio-loop", false]);
    // The paired machine's app predates webhook relays: a new webhook there is offered none.
    await navigate("/settings/triggers?new=1", field("name"));
    await value(field("kind"), "webhook");
    await until(() => requests.some((request) => request.path === "/api/triggers/relay-key" && request.source === "studio"), "the paired machine is asked for its relay key");
    await pause(300);
    assert.equal(await evaluate(`!${q('[data-slot="trigger-relay"]')}`), true, "no relay is offered where the app has none");
    await navigate("/settings/triggers", row("studio-loop"));
    // The choice is remembered, and is not the Marketplace's.
    assert.deepEqual(await evaluate(`[localStorage.getItem('adi-settings-source'), localStorage.getItem('adi-marketplace-source')]`), ['"studio"', null]);
    await reload(row("studio-loop"));
    await click('button[aria-label="Settings machine"]');
    await click('[data-slot="node-selector-menu"] [role="menuitemradio"]:nth-of-type(1)');
    await until(() => evaluate(`!!${q(row("deploy-hook"))}`), "back on this machine");
  });

  // ---- Secrets ----------------------------------------------------------------------------
  const secretRow = (name, scope = "") => `[data-slot="secret-row"][data-secret="${name}"][data-scope="${scope}"]`;
  const secretRows = () => evaluate(`Array.from(document.querySelectorAll('[data-slot="secret-row"]'),row=>(row.dataset.scope||'global')+'/'+row.dataset.secret)`);
  const secretField = (name) => `[data-slot="secret-form"] [name="${name}"]`;
  const pageText = () => evaluate("document.body.innerText");
  const everyValue = () => [...secretValues.values()];
  const noValueOnPage = async (label) => { const body = await pageText(); for (const value of everyValue()) assert(!body.includes(value.split("\n")[0]), `${label}: no stored value is on the page`); };
  // The page talks to the native API directly in the ADI app, and through the relay in the showcase.
  const primary = !requests.some((request) => request.relay);

  await check("Secrets lists names, scopes and sign-ins, and never a value", async () => {
    await navigate("/settings/secrets", secretRow("OPENAI_API_KEY"));
    assert.equal(await evaluate("document.title"), "Secrets — Settings — ADI");
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-slot="settings-nav-item"]'),item=>[item.textContent.trim(),item.getAttribute('aria-current')])`), [["Triggers", null], ["Secrets", "page"]]);
    // Global first, then project by project, each by name.
    assert.deepEqual(await secretRows(), ["global/API_KEY", "global/GMAIL_TOKEN", "global/OPENAI_API_KEY", "project-one/DEPLOY_KEY", "project-two/API_KEY"]);
    assert.equal((await text('[data-slot="secrets-count"]')).trim(), "5 secrets");
    // A project shows by its id until the machine's project list has loaded.
    await until(async () => (await evaluate(`Array.from(document.querySelectorAll('[data-slot="secret-row"] [data-slot="secret-scope"]'),badge=>badge.textContent)`)).join() === "Global,Global,Global,One,Two", "scopes are named");
    assert.match(await text(`${secretRow("GMAIL_TOKEN")} [data-slot="secret-oauth"]`), /^Google · expires in 2\dm$/);
    assert.match(await text(secretRow("DEPLOY_KEY", "project-one")), /Read-only deploy key\..*Updated 2h ago/);
    assert.deepEqual(await evaluate(`Array.from(${q('select[aria-label="Scope"]')}.options,option=>option.textContent)`), ["All scopes (5)", "Global (3)", "One (1)", "Two (1)"]);
    await tokenColor('[data-slot="settings-secrets"] [data-slot="card"]');
    await tokenColor('[data-slot="secret-new"]');
    // Filtering by scope and by words.
    await value('select[aria-label="Scope"]', "project:project-two");
    await until(async () => (await secretRows()).join() === "project-two/API_KEY", "one project's secrets");
    assert.equal((await text('[data-slot="secrets-count"]')).trim(), "1 of 5 secrets");
    assert.equal(await evaluate(`${q('[data-slot="secret-new"]')}.getAttribute('href')`), "/settings/secrets?new=1&project=project-two", "a new secret starts in the scope being looked at");
    await value('select[aria-label="Scope"]', "all");
    await value('input[aria-label="Search secrets"]', "api_key global");
    await until(async () => (await secretRows()).join() === "global/API_KEY,global/OPENAI_API_KEY", "search matches every word");
    await value('input[aria-label="Search secrets"]', "google");
    await until(async () => (await secretRows()).join() === "global/GMAIL_TOKEN", "a provider is searchable");
    await value('input[aria-label="Search secrets"]', "nothing-like-this");
    await until(() => evaluate(`!${q('[data-slot="secret-row"]')}`), "no match");
    await click('[data-slot="settings-secrets"] button:not([aria-label])');
    await until(async () => (await secretRows()).length === 5, "Reset filters brings every secret back");
    assert.deepEqual(reveals, [], "listing asks for no value");
    await noValueOnPage("the list");
    assert.equal(await text('[data-slot="settings-machine"]'), "this machine");
    await screenshot("secrets-list");
  });

  if (primary) {
    await check("a value is revealed one secret at a time, hidden again, and copied without being shown", async () => {
      await click(`${secretRow("DEPLOY_KEY", "project-one")} [data-slot="secret-reveal"]`);
      await until(() => evaluate(`!!${q(`${secretRow("DEPLOY_KEY", "project-one")} [data-slot="secret-value"]`)}`), "the value is shown in its row");
      // A multi-line value comes back whole.
      assert.equal(await text(`${secretRow("DEPLOY_KEY", "project-one")} [data-slot="secret-value"]`), "-----BEGIN KEY-----\nline-two\n-----END KEY-----");
      assert.deepEqual(reveals, [{ name: "DEPLOY_KEY", project: "project-one" }], "one deliberate request, for that secret and scope");
      assert.equal(await evaluate(`document.querySelectorAll('[data-slot="secret-value"]').length`), 1, "no other value is shown");
      assert.equal(await evaluate(`${q(`${secretRow("DEPLOY_KEY", "project-one")} [data-slot="secret-reveal"]`)}.textContent.trim()`), "Hide");
      await click(`${secretRow("DEPLOY_KEY", "project-one")} [data-slot="secret-reveal"]`);
      await until(() => evaluate(`!${q('[data-slot="secret-value"]')}`), "Hide puts it away");
      await noValueOnPage("after Hide");
      // The global API_KEY and the project's API_KEY are two secrets.
      await click(`${secretRow("API_KEY", "project-two")} [data-slot="secret-reveal"]`);
      await until(async () => (await text('[data-slot="secret-value"]')) === "project-two-api-key-value", "the project's own value");
      assert.deepEqual(reveals.at(-1), { name: "API_KEY", project: "project-two" });
      await click(`${secretRow("API_KEY", "project-two")} [data-slot="secret-reveal"]`);
      await until(() => evaluate(`!${q('[data-slot="secret-value"]')}`), "hidden again");
      const before = reveals.length;
      await click(`${secretRow("OPENAI_API_KEY")} [data-slot="secret-copy"]`);
      await until(async () => /Copied OPENAI_API_KEY to the clipboard\.|Couldn’t reach the clipboard/.test((await text('[data-slot="secret-notice"]') ?? "") + (await text('[data-slot="secret-error"]') ?? "")), "the copy is reported");
      assert.equal(reveals.length, before + 1, "copying reads the value once");
      assert.equal(await evaluate(`!${q('[data-slot="secret-value"]')}`), true, "copying shows nothing");
      await noValueOnPage("after Copy");
      // Leaving the section drops whatever was revealed.
      await click(`${secretRow("API_KEY")} [data-slot="secret-reveal"]`);
      await until(() => evaluate(`!!${q('[data-slot="secret-value"]')}`), "revealed");
      await click('[data-slot="settings-nav-item"][href="/settings/triggers"]');
      await until(() => evaluate(`!!${q('[data-slot="settings-triggers"]')}`), "Triggers opens");
      await click('[data-slot="settings-nav-item"][href="/settings/secrets"]');
      await until(() => evaluate(`!!${q(secretRow("API_KEY"))}`), "back on Secrets");
      assert.equal(await evaluate(`!${q('[data-slot="secret-value"]')}`), true, "a revealed value does not survive leaving the section");
    });
  } else {
    await check("the showcase lists and sets secrets but offers no way to read a value", async () => {
      assert.equal(await evaluate(`document.querySelectorAll('[data-slot="secret-reveal"], [data-slot="secret-copy"]').length`), 0);
      assert.match(await text('[data-slot="secrets-showcase-note"]'), /revealed and copied in the ADI app itself/);
    });
  }

  await check("a new typed secret is checked, masked while typed, and saved to its scope", async () => {
    await click('[data-slot="secret-new"]');
    await until(() => evaluate(`!!${q(secretField("name"))}`), "the new secret form opens");
    assert.equal(await evaluate("location.pathname+location.search"), "/settings/secrets?new=1");
    assert.equal(await text('[data-slot="settings-secret"] h3'), "New secret");
    const before = mutations.length;
    await click('[data-slot="secret-save"]');
    await until(async () => (await text('[data-slot="secret-form-error"]')) === "A secret name is required.", "a missing name is caught here");
    await value(secretField("name"), "2FA-code");
    await click('[data-slot="secret-save"]');
    await until(async () => /letters, digits and underscores/.test(await text('[data-slot="secret-form-error"]') ?? ""), "a name that is not an identifier is caught here");
    await value(secretField("name"), "STRIPE_KEY");
    await click('[data-slot="secret-save"]');
    await until(async () => (await text('[data-slot="secret-form-error"]')) === "Enter the value to store.", "an empty value is caught here");
    assert.equal(mutations.length, before, "nothing was sent");
    // The value is masked like a password, but it is a textarea: a multi-line value arrives whole.
    assert.deepEqual(await evaluate(`(() => {const el=${q(secretField("value"))};return [el.tagName,el.dataset.masked,getComputedStyle(el).webkitTextSecurity,el.autocomplete,el.spellcheck];})()`), ["TEXTAREA", "true", "disc", "off", false]);
    await value(secretField("value"), "sk_live_line-one\nline-two");
    await click('button[aria-label="Show the value"]');
    assert.deepEqual(await evaluate(`[${q(secretField("value"))}.dataset.masked, getComputedStyle(${q(secretField("value"))}).webkitTextSecurity]`), ["false", "none"]);
    await click('button[aria-label="Hide the value"]');
    await value(secretField("description"), "  Payments, live  ");
    assert.deepEqual(await evaluate(`Array.from(${q(secretField("project"))}.options,option=>option.textContent)`), ["Global — available to every run", "One", "Two"], "archived projects are not offered");
    await value(secretField("project"), "project-one");
    await click('[data-slot="secret-save"]');
    await until(() => evaluate(`location.search==='?secret=STRIPE_KEY&project=project-one' && !!${q('[data-slot="secret-replace-form"]')}`), "the saved secret opens");
    assert.deepEqual(lastMutation(), { path: "/api/secrets/set", body: { name: "STRIPE_KEY", project: "project-one", value: "sk_live_line-one\nline-two", description: "Payments, live" } });
    assert.equal(mutations.length, before + 1);
    assert.equal(await text('[data-slot="secret-notice"]'), "Saved STRIPE_KEY.");
    await noValueOnPage("after saving");
    // Saving over an existing name in the same scope says so first; the same name elsewhere is a new secret.
    await navigate("/settings/secrets?new=1", secretField("name"));
    await value(secretField("name"), "DEPLOY_KEY");
    assert.equal(await evaluate(`!${q('[data-slot="secret-exists"]')}`), true, "DEPLOY_KEY is a project-one secret, not a global one");
    await value(secretField("project"), "project-one");
    await until(() => evaluate(`!!${q('[data-slot="secret-exists"]')}`), "the existing secret is pointed out");
    assert.match(await text('[data-slot="secret-exists"]'), /DEPLOY_KEY already exists in One\. Saving replaces its value\./);
    assert.equal((await text('[data-slot="secret-save"]')).trim(), "Replace existing secret");
    await screenshot("secrets-new");
  });

  await check("a secret's page replaces its value, keeps its description, and reports a refusal beside the form", async () => {
    await navigate("/settings/secrets?secret=API_KEY&project=project-two", '[data-slot="secret-replace-form"]');
    await until(async () => (await text('[data-slot="settings-secret"] h3')) === "API_KEYTwo", "the secret's page names its scope");
    assert.match(await text('[data-slot="secret-facts"]'), /Injected as\$API_KEY.*ScopeProject Two.*Updated3d ago/);
    assert.equal(await evaluate(`${q('[data-slot="secret-replace-form"] [name="description"]')}.value`), "Billing sandbox key.");
    await click('[data-slot="secret-replace"]');
    await until(async () => (await text('[data-slot="secret-form-error"]')) === "Enter the new value.", "an empty replacement is caught here");
    failNextSecret = "encryption failed: key unavailable";
    await value('[data-slot="secret-replace-form"] [name="value"]', "new-sandbox-key");
    await click('[data-slot="secret-replace"]');
    await until(async () => (await text('[data-slot="secret-form-error"]')) === "encryption failed: key unavailable", "the machine's reason is shown beside the form");
    assert.equal(await evaluate(`${q('[data-slot="secret-replace-form"] [name="value"]')}.value`), "new-sandbox-key", "what was typed is kept");
    await click('[data-slot="secret-replace"]');
    await until(async () => (await text('[data-slot="secret-notice"]')) === "Replaced the value of API_KEY.", "the retry saves");
    // The description travels with every save: a save without one would clear it.
    assert.deepEqual(lastMutation(), { path: "/api/secrets/set", body: { name: "API_KEY", project: "project-two", value: "new-sandbox-key", description: "Billing sandbox key." } });
    await until(() => evaluate(`${q('[data-slot="secret-replace-form"] [name="value"]')}.value===''`), "the typed value is not left in the form");
    assert.equal(secretValues.get("||API_KEY"), "global-api-key-value", "the global secret of the same name is untouched");
    await noValueOnPage("after replacing");
    await navigate("/settings/secrets?secret=NOPE", '[data-slot="secret-missing"]');
    assert.match(await text('[data-slot="secret-missing"]'), /No secret named “NOPE” in the global scope/);
  });

  await check("an OAuth secret shows its sign-in, refreshes on the machine, and warns before a typed value drops it", async () => {
    await navigate("/settings/secrets?secret=GMAIL_TOKEN", '[data-slot="secret-oauth-actions"]');
    assert.match(await text('[data-slot="secret-facts"]'), /Signed in withGoogle · expires in 2\dm.*Access grantedhttps:\/\/www\.googleapis\.com\/auth\/gmail\.readonly email/);
    assert.match(await text('[data-slot="secret-replace-form"]'), /makes this a typed secret: its Google sign-in and refresh token are dropped/);
    await click('[data-slot="secret-refresh"]');
    await until(async () => (await text('[data-slot="secret-notice"]')) === "Refreshed GMAIL_TOKEN.", "the refresh is reported");
    assert.deepEqual(lastMutation(), { path: "/api/secrets/refresh", body: { name: "GMAIL_TOKEN", project: null } });
    await until(async () => /expires in (59m|1h)/.test(await text('[data-slot="settings-secret"] [data-slot="secret-oauth"]')), "the new expiry shows");
    if (!primary) {
      assert.equal(await evaluate(`${q('[data-slot="secret-reauthorize"]')}.disabled`), true, "sign-in can only return to the ADI app itself");
      await navigate("/settings/secrets?new=1", secretField("name"));
      await click('[data-slot="secret-form"] [data-kind="oauth"]');
      await until(() => evaluate(`${q('[data-slot="secret-authorize"]')}?.disabled===true`), "Authorize is unavailable in the showcase");
      return;
    }
    // Re-authorize asks for the access the secret already holds, in a new tab, and parks which secret it is for.
    await evaluate("window.openedTabs=[]; window.open=(...args)=>{window.openedTabs.push(args);return null;}");
    await click('[data-slot="secret-reauthorize"]');
    await until(() => evaluate("window.openedTabs.length===1"), "the sign-in opens");
    const reauth = new URL(await evaluate("window.openedTabs[0][0]"));
    assert.deepEqual([reauth.origin, reauth.pathname, reauth.searchParams.get("redirect"), reauth.searchParams.get("scope")], ["https://oauth-router.withadi.dev", "/login/google", `${origin}/settings/secrets`, "https://www.googleapis.com/auth/gmail.readonly email"]);
    assert.deepEqual(await evaluate("[window.openedTabs[0][1], window.openedTabs[0][2]]"), ["_blank", "noopener"]);
    assert.deepEqual(JSON.parse(await evaluate("localStorage.getItem('adi.oauth.pending')")), { name: "GMAIL_TOKEN", project: null, description: "Inbox, read only.", provider: "google" });
    // A new secret from a sign-in: the access is chosen, and nothing is stored until the provider answers.
    await navigate("/settings/secrets?new=1&project=project-one", secretField("name"));
    await evaluate("window.openedTabs=[]; window.open=(...args)=>{window.openedTabs.push(args);return null;}");
    const before = mutations.length;
    await click('[data-slot="secret-form"] [data-kind="oauth"]');
    await until(() => evaluate(`!!${q('[data-slot="secret-access"]')}`), "Google's access list is offered");
    assert.equal(await evaluate(`!${q(secretField("value"))}`), true, "there is no value to type");
    await value(secretField("name"), "CALENDAR_TOKEN");
    await click('[data-slot="secret-authorize"]');
    await until(async () => (await text('[data-slot="secret-form-error"]')) === "Select at least one access.", "an access must be chosen");
    await click('[data-slot="secret-access"] input[value="email"]');
    await click('[data-slot="secret-access"] input[value="https://www.googleapis.com/auth/gmail.send"]');
    await click('[data-slot="secret-authorize"]');
    await until(() => evaluate("window.openedTabs.length===1"), "the sign-in opens");
    const login = new URL(await evaluate("window.openedTabs[0][0]"));
    assert.deepEqual([login.pathname, login.searchParams.get("redirect"), login.searchParams.get("scope")], ["/login/google", `${origin}/settings/secrets`, "email https://www.googleapis.com/auth/gmail.send"]);
    assert.deepEqual(JSON.parse(await evaluate("localStorage.getItem('adi.oauth.pending')")), { name: "CALENDAR_TOKEN", project: "project-one", description: null, provider: "google" });
    assert.match(await text('[data-slot="secret-oauth-opened"]'), /The Google sign-in opened in a new tab/);
    assert.equal(mutations.length, before, "nothing is stored when the sign-in starts");
    // GitHub has no access list: its default access is asked for.
    await value(secretField("provider"), "github");
    await until(() => evaluate(`!${q('[data-slot="secret-access"]')}`), "no access list for GitHub");
    assert.equal((await text('[data-slot="secret-authorize"]')).trim(), "Authorize with GitHub");
  });

  if (primary) {
    await check("a returning sign-in stores its token once, and its fragment never stays in the address", async () => {
      // A return from the provider is a fresh document load, arriving from another site; an
      // address that differs only in its fragment would not reload anything.
      const arrive = async (fragment) => {
        await send("Page.navigate", { url: "about:blank" });
        await until(() => evaluate("location.href==='about:blank'"), "the tab left the app");
        await send("Page.navigate", { url: `${origin}/settings/secrets#${fragment}` });
        await until(() => evaluate(`location.origin===${JSON.stringify(origin)} && !!${q('main[aria-label="Settings"]')}`), "the tab returns to Secrets");
      };
      // The new tab comes back with the token in the fragment and finds the parked intent.
      await evaluate(`localStorage.setItem('adi.oauth.pending', JSON.stringify({name:'CALENDAR_TOKEN',project:'project-one',description:'Calendar',provider:'google'}))`);
      const before = mutations.length;
      await arrive("access_token=ya29.returned&refresh_token=1//refresh&expires_in=3599&scope=email+openid&provider=google");
      await until(() => evaluate(`!!${q(secretRow("CALENDAR_TOKEN", "project-one"))}`), "the returned secret is listed");
      assert.deepEqual(lastMutation(), { path: "/api/secrets/set-oauth", body: { name: "CALENDAR_TOKEN", project: "project-one", description: "Calendar", provider: "google", access_token: "ya29.returned", refresh_token: "1//refresh", expires_in: 3599, scope: "email openid" } });
      assert.equal(mutations.length, before + 1, "stored once");
      assert.equal(await evaluate("location.pathname+location.search+location.hash"), "/settings/secrets", "the token is taken out of the address");
      assert.equal(await evaluate("localStorage.getItem('adi.oauth.pending')"), null, "the parked intent is consumed");
      assert.equal(await text('[data-slot="secret-notice"]'), "Stored CALENDAR_TOKEN from Google.");
      assert.match(await text(`${secretRow("CALENDAR_TOKEN", "project-one")} [data-slot="secret-oauth"]`), /^Google · expires in 59m$/);
      const body = await pageText();
      assert(!body.includes("ya29.returned") && !body.includes("1//refresh"), "neither token is on the page");
      await reload(secretRow("CALENDAR_TOKEN", "project-one"));
      assert.equal(mutations.length, before + 1, "a reload stores nothing again");
      // A provider that says no, and a token nobody asked for.
      await evaluate(`localStorage.setItem('adi.oauth.pending', JSON.stringify({name:'DENIED_TOKEN',provider:'github'}))`);
      await arrive("error=access_denied&error_description=The+user+denied+access");
      await until(async () => (await text('[data-slot="secret-error"]')) === "The sign-in did not finish: The user denied access", "the provider's refusal is shown");
      assert.equal(await evaluate("location.hash"), "");
      assert.equal(await evaluate("localStorage.getItem('adi.oauth.pending')"), null);
      await arrive("access_token=stray-token");
      await until(async () => /no record of which secret it was for/.test(await text('[data-slot="secret-error"]') ?? ""), "a token with no parked intent is refused");
      assert.equal(await evaluate("location.hash"), "", "and still taken out of the address");
      assert.equal(mutations.length, before + 1, "neither stored anything");
    });
  }

  await check("deleting a secret asks first, names its scope, and only the confirmed delete is sent", async () => {
    await navigate("/settings/secrets?secret=STRIPE_KEY&project=project-one", '[data-slot="secret-delete"]');
    const before = mutations.length;
    await click('[data-slot="secret-delete"]');
    await until(() => evaluate(`!!${q('dialog[open][data-slot="confirm-dialog"]')}`), "the question opens");
    await until(async () => /Delete the secret “STRIPE_KEY” \(project One\) from this machine\?.*\$STRIPE_KEY.*cannot be undone/.test(await text('dialog[open]')), "the question names the secret and its scope");
    assert.equal(await evaluate(`document.activeElement.textContent`), "Cancel", "the safe answer has the focus");
    await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    await until(() => evaluate(`!${q('dialog[open]')}`), "Escape declines");
    assert.equal(mutations.length, before, "declining deletes nothing");
    await click('[data-slot="secret-delete"]');
    await click('dialog[open] [data-slot="confirm-dialog-confirm"]');
    await until(() => evaluate(`location.search==='' && !!${q(secretRow("OPENAI_API_KEY"))} && !${q(secretRow("STRIPE_KEY", "project-one"))}`), "the list returns without the secret");
    assert.deepEqual(lastMutation(), { path: "/api/secrets/remove", body: { name: "STRIPE_KEY", project: "project-one" } });
    assert.equal(mutations.length, before + 1);
    assert.equal(await text('[data-slot="secret-notice"]'), "Deleted STRIPE_KEY.");
  });

  await check("another machine's secrets are read and set through its relay", async () => {
    await click('button[aria-label="Settings machine"]');
    const studio = '[data-slot="node-selector-menu"] [role="menuitemradio"]:nth-of-type(2)';
    await until(() => evaluate(`${q(studio)}?.textContent.trim()==='studio'`), "paired machines are offered");
    await click(studio);
    await until(async () => (await secretRows()).join() === "global/STUDIO_TOKEN", "the paired machine's secrets replace this machine's");
    assert(requests.some((request) => request.path === "/api/secrets" && request.source === "studio"), "the list is read through the node relay");
    await navigate("/settings/secrets?secret=STUDIO_TOKEN", '[data-slot="secret-replace-form"]');
    await value('[data-slot="secret-replace-form"] [name="value"]', "rotated");
    await click('[data-slot="secret-replace"]');
    await until(async () => (await text('[data-slot="secret-notice"]')) === "Replaced the value of STUDIO_TOKEN.", "saved on the paired machine");
    assert.deepEqual(lastMutation(), { path: "/api/secrets/set", source: "studio", body: { name: "STUDIO_TOKEN", project: null, value: "rotated", description: "Lives on the paired machine." } });
    await click('button[aria-label="Settings machine"]');
    await click('[data-slot="node-selector-menu"] [role="menuitemradio"]:nth-of-type(1)');
    await until(() => evaluate(`!${q('[data-slot="node-selector-menu"]')}`), "back on this machine");
    await navigate("/settings/secrets", secretRow("OPENAI_API_KEY"));
  });

  await check("an unknown section says so and offers both ways on", async () => {
    await navigate("/settings/appearance", 'main[aria-label="Settings"] [role="alert"]');
    assert.match(await text('main[aria-label="Settings"] [role="alert"]'), /“appearance” is not a section of the new settings/);
    assert.equal(await evaluate("document.title"), "Settings — ADI");
    await click('main[aria-label="Settings"] [role="alert"] a[href="/settings/triggers"]');
    await until(() => evaluate(`!!${q(row("deploy-hook"))}`), "Triggers opens");
  });

  await check("phone width keeps the page, the form and the dialog inside the viewport", async () => {
    await send("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
    await navigate("/settings/triggers", row("deploy-hook"));
    const fits = () => evaluate("document.documentElement.scrollWidth<=document.documentElement.clientWidth");
    assert(await fits(), "the list does not overflow horizontally");
    assert.equal(await evaluate(`!!${q('[data-slot="settings-old-ui"]')} && ${q('[data-slot="settings-old-ui"]')}.getBoundingClientRect().right<=innerWidth`), true, "the old settings button stays on screen");
    assert.equal(await evaluate(`${q('[data-slot="global-navigation"] a[aria-label="Settings"]')}.getAttribute('aria-current')`), "page", "the Settings tab is the active tab");
    await screenshot("mobile-list");
    await navigate("/settings/triggers?trigger=deploy-hook", field("code"));
    assert(await fits(), "the definition does not overflow horizontally");
    await screenshot("mobile-definition");
    await navigate("/settings/secrets", '[data-slot="secret-row"]');
    assert(await fits(), "the secrets list does not overflow horizontally");
    await screenshot("mobile-secrets");
    await navigate("/settings/secrets?new=1", '[data-slot="secret-form"] [name="value"]');
    assert(await fits(), "the new secret form does not overflow horizontally");
    await navigate("/settings/triggers?trigger=deploy-hook", field("code"));
    await click('[data-slot="trigger-delete"]');
    await until(() => evaluate(`!!${q('dialog[open]')}`), "the question opens");
    assert(await evaluate(`(() => {const r=document.querySelector('dialog[open]').getBoundingClientRect();return r.left>=0&&r.right<=innerWidth&&r.height<=innerHeight;})()`), "the dialog fits the viewport");
    await click('dialog[open] button[type="button"]');
    await until(() => evaluate(`!${q('dialog[open]')}`), "Cancel declines");
  });

  console.log(`\n${checks.length} settings checks passed; all API requests intercepted, ${mutations.length} writes answered by the fixture.`);
} catch (error) {
  console.error(`Settings checks failed during ${stage}; ${checks.length} checks completed.`);
  if (errors.length) console.error(`Browser errors: ${JSON.stringify(errors)}`);
  if (blocked.length) console.error(`Blocked API requests: ${JSON.stringify(blocked)}`);
  if (interceptionErrors.length) console.error(`Interception errors: ${JSON.stringify(interceptionErrors)}`);
  console.error(`Last writes: ${JSON.stringify(mutations.slice(-3))}`);
  console.error(`API request paths: ${JSON.stringify(requests.slice(-25))}`);
  if (evaluate && socket?.readyState === WebSocket.OPEN) {
    try {
      console.error(JSON.stringify(await evaluate(`({path:location.pathname+location.search,title:document.title,settings:!!${q('main[aria-label="Settings"]')},text:document.querySelector('main')?.innerText.slice(0,600)})`)));
      await screenshot("failure");
    } catch (diagnosticError) { console.error(`Could not collect diagnostics: ${diagnosticError.message}`); }
  }
  throw error;
} finally {
  socket?.close();
  chrome.kill();
  await chrome.exited;
  await rm(profile, { recursive: true, force: true });
}
