/**
 * Home catalog regressions against isolated Chrome and intercepted app API fixtures.
 * Every /api/* request is fulfilled here; even lock/unlock checks cannot change real credentials.
 * Run: bun frontend/tests/home-apps.browser.mjs
 * Optional: SHOWCASE_URL, CHROME_BIN, BROWSER_SCREENSHOTS_DIR
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
const profile = await mkdtemp(resolve(tmpdir(), "design-system-home-tests-"));
const chrome = Bun.spawn([
  chromePath, "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`,
  "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "about:blank",
], { stdout: "ignore", stderr: "ignore" });

const local = { dashboards: [
  { id: "design-system", name: "Design System", description: "Shared component gallery", host: "design-system.adi", frontend_running: true, archived_at: null },
  { id: "sleeping-app", name: "Sleeping app", description: "A local app that wakes when opened", host: "sleeping-app.adi", frontend_running: false, archived_at: null },
  { id: "archived", name: "Archived app", host: "archived.adi", frontend_running: false, archived_at: 1000 },
] };
const fleet = { nodes: [
  { node: "server-alpha", locked: false, error: null, dashboards: [
    { id: "crm", name: "CRM", description: "Customer accounts", service: "crm", url: "http://crm.server-alpha.n.adi/", allowed: true, running: false },
    { id: "design-system", name: "Design System", service: "design-system", url: "http://design-system.server-alpha.n.adi/", allowed: true, running: true },
    { id: "restricted", name: "Restricted app", service: "restricted", url: "http://restricted.server-alpha.n.adi/", allowed: false, running: true },
    { id: "no-address", name: "Unroutable app", service: null, url: null, allowed: false, running: false },
  ] },
  { node: "server-beta", locked: false, error: null, dashboards: [
    { id: "crm", name: "CRM", description: "Customer accounts on another machine", service: "crm", url: "http://crm.server-beta.n.adi/", allowed: true, running: true },
  ] },
  { node: "locked-machine", locked: true, error: null, dashboards: [] },
  { node: "failed-machine", locked: false, error: "The machine could not be reached.", dashboards: [] },
] };
const requests = [];
const errors = [];
const mockErrors = [];
const checks = [];
let failLocal = false;
let failFleet = false;
let failForget = false;
let holdUnlock = false;
let pendingUnlock;
const unlockBodies = [];
const forgetBodies = [];
const validPassword = " fixture-unlock-password "; // Password whitespace must be preserved verbatim.
const wrongPassword = "fixture-incorrect-password";
const unlockedDashboard = { id: "remote-notes", name: "Remote notes", description: "An app revealed after unlocking", service: "remote-notes", url: "http://remote-notes.locked-machine.n.adi/", allowed: true, running: true };
let socket;
let send;
let evaluate;
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const q = (selector) => `document.querySelector(${JSON.stringify(selector)})`;
const grid = 'main[aria-label="Home"] ul[aria-label="Applications"]';
const links = `${grid} [data-slot="home-app-link"]`;
const filter = (machine) => `[data-slot="home-machine-filter"][data-machine="${machine}"]`;
const app = (source, name) => `${grid} li[data-source="${source}"] [aria-label^="${name} · "]`;
const unlockButton = 'button[aria-label="Unlock locked-machine"]';
const unlockDialog = 'dialog[aria-label="Unlock locked-machine"]';
const unlockSubmit = `${unlockDialog} button[type="submit"]`;
const passwordInput = `${unlockDialog} [aria-label="Password"]`;
const usernameInput = `${unlockDialog} [aria-label="Username"]`;
const lockButton = (machine) => `button[aria-label="Lock ${machine}"]`;

async function until(fn, description, timeout = 10000) {
  const deadline = Date.now() + timeout;
  let last;
  do {
    try { const result = await fn(); if (result) return result; } catch (error) { last = error; }
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
    // Reloads may cancel an in-flight fixture response.
    if (!/Invalid InterceptionId|Invalid interceptionId|Invalid requestId|No resource with given identifier|interception.*not found/i.test(error.message)) throw error;
  }
}

async function intercept({ requestId, request }) {
  const url = new URL(request.url);
  requests.push({ method: request.method, path: fixturePath(url.pathname) });
  if (request.method === "GET" && fixturePath(url.pathname) === "/api/app/dashboards") {
    // Malformed catalogs exercise the query error path without adding expected HTTP console errors.
    await fulfill(requestId, failLocal ? { error: "Fixture local listing failure" } : local);
  } else if (request.method === "GET" && fixturePath(url.pathname) === "/api/app/fleet/dashboards") {
    await fulfill(requestId, failFleet ? { error: "Fixture fleet listing failure" } : fleet);
  } else if (request.method === "POST" && fixturePath(url.pathname) === "/api/app/fleet/dashboards/unlock") {
    const body = JSON.parse(request.postData ?? "{}");
    unlockBodies.push(body);
    if (holdUnlock) {
      if (pendingUnlock) mockErrors.push("Duplicate unlock request while an unlock is pending.");
      pendingUnlock = { requestId, body };
      return;
    }
    if (body.password !== validPassword) {
      await fulfill(requestId, { ok: false, error: "Invalid username or password." });
    } else {
      const machine = fleet.nodes.find((item) => item.node === body.node);
      machine.locked = false;
      machine.dashboards = [unlockedDashboard];
      await fulfill(requestId, fleet);
    }
  } else if (request.method === "POST" && fixturePath(url.pathname) === "/api/app/fleet/dashboards/forget") {
    const body = JSON.parse(request.postData ?? "{}");
    forgetBodies.push(body);
    if (failForget) {
      await fulfill(requestId, { ok: false, error: "The saved access could not be removed." });
    } else {
      const machine = fleet.nodes.find((item) => item.node === body.petname);
      machine.locked = true;
      machine.dashboards = [];
      await fulfill(requestId, fleet);
    }
  } else {
    mockErrors.push(`Unexpected API request: ${request.method} ${url.pathname}`);
    await fulfill(requestId, { error: "Only Home catalog reads and fixture lock/unlock requests are allowed." });
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
    await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  }
  async function type(text) {
    await fill('[aria-label="Search apps"]', text);
  }
  async function fill(selector, text) {
    await click(selector);
    await evaluate(`${q(selector)}.select()`);
    await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Backspace", code: "Backspace", windowsVirtualKeyCode: 8 });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Backspace", code: "Backspace", windowsVirtualKeyCode: 8 });
    if (text) await send("Input.insertText", { text });
    await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  }
  async function key(key, code, windowsVirtualKeyCode) {
    await send("Input.dispatchKeyEvent", { type: "keyDown", key, code, windowsVirtualKeyCode });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode });
    await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  }
  async function assertPasswordIsNotStored() {
    const retained = await evaluate(`JSON.stringify({local: {...localStorage},session: {...sessionStorage},url:location.href,body:document.body.innerText}).includes(${JSON.stringify(validPassword)}) || JSON.stringify({local: {...localStorage},session: {...sessionStorage},url:location.href,body:document.body.innerText}).includes(${JSON.stringify(wrongPassword)})`);
    assert.equal(retained, false, "credentials are absent from browser storage, URL and rendered text");
  }
  async function check(name, fn) { await fn(); checks.push(name); console.log(`PASS ${name}`); }
  async function ready(count = 7) {
    await until(() => evaluate(`${q(grid)}?.getAttribute('aria-busy')==='false' && ${q('[data-slot="home-app-count"]')}?.textContent==='${count}'`), "Home catalog is settled");
  }
  async function setTheme(dark) {
    const toggle = '[data-slot="theme-toggle"]';
    if (await evaluate(`${q(toggle)}.getAttribute('aria-checked')`) !== String(dark)) await click(toggle);
    await until(() => evaluate(`document.documentElement.classList.contains('dark')===${dark}`), "theme is applied");
    // Transition colors interpolate through oklab before settling on the oklch theme token.
    await until(() => evaluate(`getComputedStyle(${q('[data-slot="home-new-chat"]')}).backgroundColor.startsWith('oklch(')`), "theme color transition settles");
  }

  await Promise.all([send("Runtime.enable"), send("Page.enable"), send("Log.enable")]);
  // No continueRequest branch: no API call can escape these fixtures, including future writes.
  await send("Fetch.enable", { patterns: [
    { urlPattern: "*/api/*", requestStage: "Request" },
    { urlPattern: "*://app.adi/api/*", requestStage: "Request" },
  ] });
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
  await send("Page.navigate", { url: `${origin}/` });
  await ready();

  await check("Home combines local and remote apps with source labels and accurate counts", async () => {
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 7);
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-slot="home-machine-filter"]'), el => ({machine:el.dataset.machine,label:el.textContent.trim()}))`), [
      { machine: "*", label: "All machines7" }, { machine: "", label: "This machine2" },
      { machine: "server-alpha", label: "server-alpha4" }, { machine: "server-beta", label: "server-beta1" },
    ]);
    assert.equal(await evaluate(`${q(app("", "Sleeping app"))}.textContent.includes('This machine')`), true);
    assert.equal(await evaluate(`${q(app("server-alpha", "CRM"))}.textContent.includes('server-alpha')`), true);
    assert.equal(await evaluate(`${q(grid)}.textContent.includes('Archived app')`), false);
    assert.equal(await evaluate(`${q('[data-slot="home-new-chat"]')}.textContent.trim()`), "New chat");
  });

  await check("duplicate remote IDs, stopped apps and remote gallery copies keep their own URLs", async () => {
    const href = (source, name) => evaluate(`${q(app(source, name))}.getAttribute('href')`);
    assert.equal(await href("", "Design System"), "/library");
    assert.equal(await href("", "Sleeping app"), "http://sleeping-app.adi/");
    assert.equal(await href("server-alpha", "CRM"), "http://crm.server-alpha.n.adi/");
    assert.equal(await href("server-beta", "CRM"), "http://crm.server-beta.n.adi/");
    assert.equal(await href("server-alpha", "Design System"), "http://design-system.server-alpha.n.adi/");
    assert.equal(await evaluate(`document.querySelectorAll('${links}').length`), 6);
  });

  await check("access-required and unroutable apps stay visible without requesting grants", async () => {
    assert.equal(await evaluate(`${q(app("server-alpha", "Restricted app"))}.getAttribute('href')`), "http://app.server-alpha.n.adi/");
    assert.equal(await evaluate(`${q(app("server-alpha", "Restricted app"))}.textContent.includes('Access required')`), true);
    assert.equal(await evaluate(`${q(app("server-alpha", "Unroutable app"))}.dataset.slot`), "home-app-unavailable");
    assert.equal(await evaluate(`${q(app("server-alpha", "Unroutable app"))}.textContent.includes('No app address is available.')`), true);
    assert.equal(requests.some((request) => request.method !== "GET"), false);
  });

  await check("locked and unavailable machines expose their status and control panel links", async () => {
    const states = await evaluate(`Array.from(document.querySelectorAll('[aria-label="Unavailable machines"] li'), el => ({text:el.textContent.trim(),href:el.querySelector('a').href,title:el.querySelector('a').title}))`);
    assert.deepEqual(states.map(({ href, title }) => ({ href, title })), [
      { href: "http://app.locked-machine.n.adi/", title: "Unlock this machine to see its apps." },
      { href: "http://app.failed-machine.n.adi/", title: "The machine could not be reached." },
    ]);
    assert.match(states[0].text, /locked-machine.*Locked/);
    assert.match(states[1].text, /failed-machine.*Unavailable/);
    assert.equal(await evaluate(`${q(unlockButton)}.textContent.trim()`), "Unlock");
  });

  await check("unlock opens a modal with username and password fields; Cancel clears it without a request", async () => {
    const before = unlockBodies.length;
    await click(unlockButton);
    await until(() => evaluate(`${q(unlockDialog)}?.open`), "unlock dialog opens");
    assert.equal(await evaluate(`${q(unlockDialog)}.matches(':modal')`), true);
    assert.equal(await evaluate(`${q(usernameInput)}.value`), "adi");
    assert.equal(await evaluate(`${q(passwordInput)}.type`), "password");
    assert.equal(await evaluate(`${q(passwordInput)}.value`), "");
    assert.equal(await evaluate(`${q(passwordInput)}.required`), true);
    assert.equal(await evaluate(`${q(unlockDialog)}.contains(document.activeElement)`), true);
    await fill(passwordInput, wrongPassword);
    await click(`${unlockDialog} button[type="button"]`);
    await until(() => evaluate(`!${q(unlockDialog)}?.open`), "Cancel closes unlock dialog");
    assert.equal(unlockBodies.length, before);
    assert.equal(await evaluate(`document.activeElement===${q(unlockButton)}`), true);
    await assertPasswordIsNotStored();
    await click(unlockButton);
    assert.equal(await evaluate(`${q(passwordInput)}.value`), "");
    await key("Escape", "Escape", 27);
  });

  await check("Escape dismisses unlock and restores focus without retaining the password", async () => {
    const before = unlockBodies.length;
    await click(unlockButton);
    await fill(passwordInput, wrongPassword);
    await key("Escape", "Escape", 27);
    await until(() => evaluate(`!${q(unlockDialog)}?.open`), "Escape closes unlock dialog");
    assert.equal(await evaluate(`document.activeElement===${q(unlockButton)}`), true);
    assert.equal(unlockBodies.length, before);
    await click(unlockButton);
    assert.equal(await evaluate(`${q(passwordInput)}.value`), "");
    await key("Escape", "Escape", 27);
    await assertPasswordIsNotStored();
  });

  await check("empty passwords cannot submit and keyboard focus stays inside the modal", async () => {
    const before = unlockBodies.length;
    await click(unlockButton);
    await click(unlockSubmit);
    assert.equal(await evaluate(`${q(passwordInput)}.validity.valueMissing`), true);
    assert.equal(unlockBodies.length, before);
    for (let index = 0; index < 6; index += 1) {
      await key("Tab", "Tab", 9);
      assert.equal(await evaluate(`${q(unlockDialog)}.contains(document.activeElement)`), true);
    }
    await key("Escape", "Escape", 27);
  });

  await check("incorrect credentials show the API error and keep the machine locked", async () => {
    await click(unlockButton);
    await fill(usernameInput, "operator");
    await fill(passwordInput, wrongPassword);
    await click(unlockSubmit);
    await until(() => evaluate(`${q(`${unlockDialog} [role="alert"]`)}?.textContent.includes('Invalid username or password.')`), "unlock error is visible");
    assert.deepEqual(unlockBodies.at(-1), { node: "locked-machine", username: "operator", password: wrongPassword });
    assert.equal(await evaluate(`${q(unlockDialog)}.open`), true);
    assert.equal(await evaluate(`${q(unlockSubmit)}.disabled`), false);
    assert.equal(await evaluate(`!!${q(app("locked-machine", "Remote notes"))}`), false);
    assert.equal(await evaluate(`${q('[data-slot="home-app-count"]')}.textContent`), "7");
    await assertPasswordIsNotStored();
    await key("Escape", "Escape", 27);
  });

  await check("Cancel dismisses a pending unlock and reopening starts with empty credentials", async () => {
    holdUnlock = true;
    await click(unlockButton);
    await fill(passwordInput, wrongPassword);
    await click(unlockSubmit);
    await until(() => pendingUnlock, "canceled unlock request is held by fixture");
    await click(`${unlockDialog} button[type="button"]`);
    await until(() => evaluate(`!${q(unlockDialog)}?.open`), "pending dialog closes on Cancel");
    assert.equal(await evaluate(`document.activeElement===${q(unlockButton)}`), true);
    await fulfill(pendingUnlock.requestId, { ok: false, error: "A canceled request must not display this error." });
    pendingUnlock = undefined;
    holdUnlock = false;
    await click(unlockButton);
    assert.equal(await evaluate(`${q(passwordInput)}.value`), "");
    assert.equal(await evaluate(`!!${q(`${unlockDialog} [role="alert"]`)}`), false);
    assert.equal(await evaluate(`${q(usernameInput)}.disabled`), false);
    await key("Escape", "Escape", 27);
    await assertPasswordIsNotStored();
  });

  await check("unlock disables pending submission and reveals returned apps immediately without reload", async () => {
    holdUnlock = true;
    const before = unlockBodies.length;
    const navigation = await evaluate("performance.timeOrigin");
    await click(unlockButton);
    await fill(usernameInput, "adi");
    await fill(passwordInput, validPassword);
    await click(unlockSubmit);
    await until(() => pendingUnlock, "unlock request is held by fixture");
    assert.equal(await evaluate(`${q(unlockSubmit)}.disabled`), true);
    assert.equal(await evaluate(`${q(usernameInput)}.disabled`), true);
    assert.equal(await evaluate(`${q(passwordInput)}.disabled`), true);
    assert.equal(await evaluate(`${q(unlockSubmit)}.textContent.trim()`), "Unlocking…");
    await evaluate(`${q(unlockDialog)}.querySelector('form').requestSubmit()`);
    await key("Enter", "Enter", 13);
    await pause(100);
    assert.equal(unlockBodies.length, before + 1, "pending unlock cannot be submitted twice");
    assert.deepEqual(pendingUnlock.body, { node: "locked-machine", username: "adi", password: validPassword });
    const machine = fleet.nodes.find((item) => item.node === "locked-machine");
    machine.locked = false;
    machine.dashboards = [unlockedDashboard];
    await fulfill(pendingUnlock.requestId, fleet);
    pendingUnlock = undefined;
    holdUnlock = false;
    await ready(8);
    await until(() => evaluate(`!${q(unlockDialog)}?.open`), "successful unlock closes dialog");
    assert.equal(await evaluate(`${q(app("locked-machine", "Remote notes"))}.getAttribute('href')`), unlockedDashboard.url);
    assert.equal(await evaluate(`${q(filter("locked-machine"))}.textContent.trim()`), "locked-machine1");
    assert.equal(await evaluate(`!!${q(unlockButton)}`), false);
    await until(() => evaluate(`document.activeElement===${q(filter("locked-machine"))}`), "successful unlock focuses the machine filter");
    assert.equal(await evaluate("performance.timeOrigin"), navigation, "catalog updates without a document reload");
    await assertPasswordIsNotStored();
  });

  await check("locking reports a failed request while preserving the machine's apps", async () => {
    failForget = true;
    await click(lockButton("locked-machine"));
    await until(() => evaluate(`Array.from(document.querySelectorAll('[role="alert"]')).some(el=>el.textContent.includes('The saved access could not be removed.'))`), "lock error is visible");
    assert.deepEqual(forgetBodies.at(-1), { petname: "locked-machine" });
    assert.equal(await evaluate(`!!${q(app("locked-machine", "Remote notes"))}`), true);
    assert.equal(await evaluate(`${q(lockButton("locked-machine"))}.disabled`), false);
    assert.equal(await evaluate(`${q('[data-slot="home-app-count"]')}.textContent`), "8");
    failForget = false;
  });

  await check("locking forgets saved machine access and updates app counts immediately", async () => {
    await click(filter("locked-machine"));
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 1);
    await click(lockButton("locked-machine"));
    await ready();
    assert.deepEqual(forgetBodies.at(-1), { petname: "locked-machine" });
    assert.equal(await evaluate(`!!${q(app("locked-machine", "Remote notes"))}`), false);
    assert.equal(await evaluate(`!!${q(unlockButton)}`), true);
    assert.equal(await evaluate(`${q(filter("*"))}.getAttribute('aria-pressed')`), "true");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 7);
    await click(unlockButton);
    assert.equal(await evaluate(`${q(passwordInput)}.value`), "");
    await key("Escape", "Escape", 27);
    await assertPasswordIsNotStored();
  });

  await check("machine filters and search compose across app names, descriptions and machines", async () => {
    await click(filter("server-alpha"));
    assert.equal(await evaluate(`${q(filter("server-alpha"))}.getAttribute('aria-pressed')`), "true");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 4);
    await type("CRM");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 1);
    await click(filter("*"));
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 2);
    await type("server-beta");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 1);
    await type("shared component");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 1);
    await type("missing application");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 0);
    assert.equal(await evaluate(`${q('main[aria-label="Home"] [role="status"]')}.textContent`), "No apps found for “missing application”.");
    await click('[aria-label="Clear app search"]');
    assert.equal(await evaluate(`document.activeElement.getAttribute('aria-label')`), "Search apps");
    await click(filter(""));
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 2);
    await click(filter("*"));
  });

  await check("a local catalog failure preserves remote apps and Retry recovers independently", async () => {
    failLocal = true;
    await send("Page.reload", { ignoreCache: true });
    await ready(5);
    await until(() => evaluate(`Array.from(document.querySelectorAll('[role="alert"]')).some(el=>el.textContent.startsWith('This machine:'))`), "local failure is visible");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 5);
    assert.equal(await evaluate(`!!${q(app("server-beta", "CRM"))}`), true);
    failLocal = false;
    await click('[role="alert"] button');
    await ready();
    assert.equal(await evaluate(`document.querySelectorAll('[role="alert"]').length`), 0);
  });

  await check("a fleet catalog failure preserves local apps and Retry restores remote machines", async () => {
    failFleet = true;
    await send("Page.reload", { ignoreCache: true });
    await ready(2);
    await until(() => evaluate(`Array.from(document.querySelectorAll('[role="alert"]')).some(el=>el.textContent.startsWith('Other machines:'))`), "fleet failure is visible");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 2);
    assert.equal(await evaluate(`${q(app("", "Design System"))}.getAttribute('href')`), "/library");
    failFleet = false;
    await click('[role="alert"] button');
    await ready();
    assert.equal(await evaluate(`document.querySelectorAll('[role="alert"]').length`), 0);
  });

  await check("mobile app tiles wrap into multiple columns without horizontal overflow", async () => {
    for (const width of [390, 430]) {
      await send("Emulation.setDeviceMetricsOverride", { width, height: 900, deviceScaleFactor: 1, mobile: true });
      await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
      const layout = await evaluate(`(() => {const list=${q(grid)};const rect=list.getBoundingClientRect();return {columns:getComputedStyle(list).gridTemplateColumns.split(/\\s+/).length,left:rect.left,right:rect.right,overflow:document.documentElement.scrollWidth>innerWidth};})()`);
      assert.ok(layout.columns >= 2, `${width}px viewport has multiple app columns`);
      assert.ok(layout.left >= 0 && layout.right <= width + 1, `${width}px app grid fits the viewport`);
      assert.equal(layout.overflow, false, `${width}px page does not overflow horizontally`);
    }
  });

  await check("unlock form fits a mobile viewport with reachable credentials and actions", async () => {
    await send("Emulation.setDeviceMetricsOverride", { width: 390, height: 900, deviceScaleFactor: 1, mobile: true });
    await click(unlockButton);
    const layout = await evaluate(`(() => {const dialog=${q(unlockDialog)};const rect=dialog.getBoundingClientRect();return {left:rect.left,right:rect.right,top:rect.top,bottom:rect.bottom,overflow:document.documentElement.scrollWidth>innerWidth};})()`);
    assert.ok(layout.left >= 0 && layout.right <= 391, "dialog fits viewport width");
    assert.ok(layout.top >= 0 && layout.bottom <= 901, "dialog actions fit viewport height");
    assert.equal(layout.overflow, false);
    assert.equal(await evaluate(`${q(passwordInput)}.value`), "");
    if (process.env.BROWSER_SCREENSHOTS_DIR) {
      await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
      const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
      await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, "design-system-home-unlock-mobile.png"), Buffer.from(screenshot.data, "base64"));
    }
    await key("Escape", "Escape", 27);
  });

  await check("light and dark Home apply theme tokens with rendered components and no errors", async () => {
    for (const dark of [false, true]) {
      await setTheme(dark);
      const colors = await evaluate(`(() => {const button=${q('[data-slot="home-new-chat"]')};const card=${q(grid)}.querySelector('[data-slot="card"]');return {button:getComputedStyle(button).backgroundColor,foreground:getComputedStyle(card).color,muted:getComputedStyle(card.querySelector('span')).backgroundColor,cards:document.querySelectorAll('${grid} [data-slot="card"]').length};})()`);
      assert.match(colors.button, /^oklch\(/);
      assert.match(colors.muted, /^oklch\(/);
      assert.notEqual(colors.button, "rgba(0, 0, 0, 0)");
      assert.equal(colors.cards, 7);
      assert.equal(await evaluate("document.documentElement.scrollWidth<=innerWidth"), true);
      if (process.env.BROWSER_SCREENSHOTS_DIR) {
        await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
        await evaluate("window.scrollTo({top:0,behavior:'instant'})");
        const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
        await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, `design-system-home-${dark ? "dark" : "light"}.png`), Buffer.from(screenshot.data, "base64"));
      }
    }
  });

  assert.deepEqual(mockErrors, [], "all API requests stayed within the Home fixture contract");
  assert.deepEqual(errors, [], "zero browser exceptions or console errors");
  assert.equal(requests.filter((request) => request.method !== "GET").every((request) => request.method === "POST" && ["/api/app/fleet/dashboards/unlock", "/api/app/fleet/dashboards/forget"].includes(request.path)), true, "only expected lock/unlock mutations were requested");
  console.log(`\n${checks.length} Home browser scenarios passed; all API requests intercepted; zero real mutations, exceptions or console errors.`);
} catch (error) {
  console.error(`Home browser check failed after ${checks.length} completed scenarios.`);
  if (errors.length) console.error(`Browser errors: ${errors.join("\n")}`);
  if (mockErrors.length) console.error(`Fixture errors: ${mockErrors.join("\n")}`);
  console.error(`Intercepted requests: ${JSON.stringify(requests)}`);
  if (evaluate && socket?.readyState === WebSocket.OPEN) {
    try { console.error(JSON.stringify(await evaluate(`({path:location.pathname,title:document.title,apps:document.querySelectorAll('${grid} > li').length,alerts:Array.from(document.querySelectorAll('[role="alert"]'),el=>el.textContent),viteError:!!document.querySelector('vite-error-overlay')})`))); }
    catch (diagnosticError) { console.error(`Could not collect diagnostics: ${diagnosticError.message}`); }
  }
  throw error;
} finally {
  socket?.close();
  chrome.kill();
  await chrome.exited;
  await rm(profile, { recursive: true, force: true });
}
