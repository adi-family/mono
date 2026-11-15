/**
 * Home catalog regressions against isolated Chrome and intercepted app API fixtures.
 * Every /api/* request is fulfilled here, and Home is expected to make reads only.
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
let socket;
let send;
let evaluate;
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const q = (selector) => `document.querySelector(${JSON.stringify(selector)})`;
const grid = 'main[aria-label="Home"] ul[aria-label="Applications"]';
const links = `${grid} [data-slot="home-app-link"]`;
const app = (source, name) => `${grid} li[data-source="${source}"] [aria-label^="${name} · "]`;
const selector = '[data-slot="app-topbar"] [data-slot="node-selector"]';
const menu = '[data-slot="node-selector-menu"]';
const menuItem = (name) => `[...document.querySelectorAll('${menu} [role="menuitemcheckbox"]')].find(node=>node.textContent.trim()===${JSON.stringify(name)})`;
const eventLog = `JSON.parse(sessionStorage.getItem('adi.event-log.v1') ?? '{"events":[]}').events`;

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
  } else if (request.method === "GET" && fixturePath(url.pathname) === "/api/app/fleet/nodes") {
    // The topbar machine selector lists the paired fleet on every page, Home included.
    await fulfill(requestId, { nodes: fleet.nodes.map(({ node, locked }) => ({ node, locked })) });
  } else {
    mockErrors.push(`Unexpected API request: ${request.method} ${url.pathname}`);
    await fulfill(requestId, { error: "Only Home catalog reads are allowed." });
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
  /** Ticks or unticks machines in the topbar selector, the only machine control Home has. */
  async function toggleMachines(...names) {
    await click(selector);
    await until(() => evaluate(`!!${q(menu)}`), "machine selector opens");
    for (const name of names) {
      const before = await evaluate(`${menuItem(name)}.getAttribute('aria-checked')`);
      await evaluate(`${menuItem(name)}.click()`);
      await until(() => evaluate(`${menuItem(name)}.getAttribute('aria-checked')!==${JSON.stringify(before)}`), `${name} is toggled`);
    }
    await key("Escape", "Escape", 27);
    await until(() => evaluate(`!${q(menu)}`), "machine selector closes");
  }
  async function check(name, fn) { await fn(); checks.push(name); console.log(`PASS ${name}`); }
  /** Reloads and waits for the new document: the old one would still satisfy a `ready()` that expects the same count. */
  async function reload() {
    await evaluate("window.staleDocument = true");
    await send("Page.reload", { ignoreCache: true });
    await until(() => evaluate(`!window.staleDocument && !!${q(grid)}`), "the reloaded Home is mounted");
  }
  async function ready(count = 2) {
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

  await check("Home has an absolute expand control without a section sidebar", async () => {
    const control = '[data-slot="workspace-expand"]';
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('${control}')).position`), 'absolute');
    assert.equal(await evaluate(`document.querySelector('[data-slot="app-secondary-navigation"]').hidden`), true);
    await click('[aria-label="Expand workspace"]');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('[data-slot="app-sidebar"]')).display`), 'none');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('[data-slot="app-topbar"]')).display`), 'none');
    assert.deepEqual(await evaluate(`(() => {const r=document.querySelector('[data-slot="app-content"]').getBoundingClientRect();return [r.x,r.y,r.width,r.height]})()`), [0,0,1280,1000]);
    await click('[aria-label="Restore navigation"]');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('[data-slot="app-topbar"]')).display`), 'flex');
  });

  await check("Home has no machine filter, lock or unlock controls of its own", async () => {
    const home = 'main[aria-label="Home"]';
    assert.equal(await evaluate(`document.querySelectorAll('${home} nav, ${home} [data-slot="home-machine-filter"], ${home} [aria-label="Unavailable machines"]').length`), 0);
    assert.equal(await evaluate(`[...document.querySelectorAll('${home} button')].filter(el=>/^(Lock|Unlock)\\b/.test(el.getAttribute('aria-label') ?? el.textContent.trim())).length`), 0);
    assert.equal(await evaluate(`${q(home)}.innerText.includes('All machines')`), false);
    assert.equal(await evaluate(`!!${q('dialog')}`), false);
    assert.equal(await evaluate(`${q('[data-slot="home-new-chat"]')}.textContent.trim()`), "New chat");
  });

  await check("a fresh visit lists this machine's apps, matching the topbar selector", async () => {
    assert.equal(await evaluate(`${q(selector)}.textContent.trim()`), "This machine");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 2);
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li[data-source=""]').length`), 2);
    assert.equal(await evaluate(`${q(app("", "Sleeping app"))}.textContent.includes('This machine')`), true);
    assert.equal(await evaluate(`${q(grid)}.textContent.includes('Archived app')`), false);
    assert.equal(await evaluate(`${q('[data-slot="home-app-count"]')}.textContent`), "2");
  });

  await check("ticking machines in the topbar selector adds their apps without leaving Home", async () => {
    await click(selector);
    await until(() => evaluate(`${q(`${menu} p`)}?.textContent==='Apps and sessions from'`), "the selector says it decides Home's apps");
    await key("Escape", "Escape", 27);
    await until(() => evaluate(`!${q(menu)}`), "machine selector closes");
    await toggleMachines("server-alpha");
    await ready(6);
    assert.equal(await evaluate(`${q(selector)}.textContent.trim()`), "2 machines");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li[data-source="server-alpha"]').length`), 4);
    await toggleMachines("server-beta");
    await ready(7);
    assert.equal(await evaluate(`${q(app("server-beta", "CRM"))}.textContent.includes('server-beta')`), true);
    assert.equal(await evaluate("location.pathname + location.search"), "/");
    assert.deepEqual(await evaluate("JSON.parse(localStorage.getItem('adi-session-sources'))"), { local: true, nodes: ["server-alpha", "server-beta"] });
  });

  await check("unticking a machine, this one included, removes its apps and stays on Home", async () => {
    await toggleMachines("This machine");
    await ready(5);
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li[data-source=""]').length`), 0);
    assert.equal(await evaluate(`!!${q('main[aria-label="Home"]')} && location.pathname + location.search`), "/");
    await toggleMachines("server-alpha");
    await ready(1);
    assert.equal(await evaluate(`${q(selector)}.textContent.trim()`), "server-beta");
    assert.equal(await evaluate("location.pathname + location.search"), "/");
    await toggleMachines("This machine", "server-alpha");
    await ready(7);
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
  });

  await check("an unreachable chosen machine is recorded in the event log, not on the page", async () => {
    assert.equal(await evaluate(`${eventLog}.some(event=>event.area==='Apps')`), false, "machines that are not chosen report nothing");
    await toggleMachines("failed-machine");
    await until(() => evaluate(`${eventLog}.some(event=>event.area==='Apps' && event.source==='failed-machine' && event.level==='error' && event.message==='The machine could not be reached.')`), "the failure reaches the event log");
    await ready(7);
    assert.equal(await evaluate(`document.querySelectorAll('main[aria-label="Home"] [role="alert"]').length`), 0);
    assert.equal(await evaluate(`${q('main[aria-label="Home"]')}.innerText.includes('failed-machine')`), false);
    await toggleMachines("failed-machine");
  });

  await check("a locked machine cannot be chosen and none of its state shows on Home", async () => {
    await click(selector);
    await until(() => evaluate(`!!${q(menu)}`), "machine selector opens");
    assert.equal(await evaluate(`${menuItem("locked-machine")}.disabled`), true);
    await key("Escape", "Escape", 27);
    await until(() => evaluate(`!${q(menu)}`), "machine selector closes");
    assert.equal(await evaluate(`${q('main[aria-label="Home"]')}.innerText.includes('locked-machine')`), false);
  });

  await check("search composes with the chosen machines across names, descriptions and machines", async () => {
    await type("CRM");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 2);
    await toggleMachines("server-beta");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 1);
    await toggleMachines("server-beta");
    await type("server-beta");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 1);
    await type("shared component");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 1);
    await type("missing application");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 0);
    assert.equal(await evaluate(`${q('main[aria-label="Home"] [role="status"]')}.textContent`), "No apps found for “missing application”.");
    await click('[aria-label="Clear app search"]');
    assert.equal(await evaluate(`document.activeElement.getAttribute('aria-label')`), "Search apps");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 7);
  });

  await check("a local catalog failure preserves remote apps and Retry recovers independently", async () => {
    failLocal = true;
    await reload();
    await ready(5);
    await until(() => evaluate(`Array.from(document.querySelectorAll('[role="alert"]')).some(el=>el.textContent.startsWith('This machine:'))`), "local failure is visible");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 5);
    assert.equal(await evaluate(`!!${q(app("server-beta", "CRM"))}`), true);
    failLocal = false;
    await click('[role="alert"] button');
    await ready(7);
    assert.equal(await evaluate(`document.querySelectorAll('[role="alert"]').length`), 0);
  });

  await check("a fleet catalog failure preserves local apps and Retry restores remote machines", async () => {
    failFleet = true;
    await reload();
    await ready(2);
    await until(() => evaluate(`Array.from(document.querySelectorAll('[role="alert"]')).some(el=>el.textContent.startsWith('Other machines:'))`), "fleet failure is visible");
    assert.equal(await evaluate(`document.querySelectorAll('${grid} > li').length`), 2);
    assert.equal(await evaluate(`${q(app("", "Design System"))}.getAttribute('href')`), "/library");
    failFleet = false;
    await click('[role="alert"] button');
    await ready(7);
    assert.equal(await evaluate(`document.querySelectorAll('[role="alert"]').length`), 0);
  });

  await check("a fleet catalog failure is not shown while only this machine is chosen", async () => {
    await toggleMachines("server-alpha", "server-beta");
    await ready(2);
    failFleet = true;
    const before = requests.length;
    await reload();
    await ready(2);
    await until(() => requests.slice(before).some((request) => request.path === "/api/app/fleet/dashboards"), "the failing fleet catalog was read");
    await pause(150);
    assert.equal(await evaluate(`document.querySelectorAll('main[aria-label="Home"] [role="alert"]').length`), 0);
    assert.equal(await evaluate(`${q('main[aria-label="Home"] [role="status"]')}.textContent`), "2 apps.");
    failFleet = false;
    await reload();
    await ready(2);
    await toggleMachines("server-alpha", "server-beta");
    await ready(7);
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
  assert.deepEqual(requests.filter((request) => request.method !== "GET"), [], "Home only reads");
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
