/**
 * Read-only smoke against the activated primary ADI app using an isolated Chrome profile.
 * Run: bun frontend/tests/primary-app.browser.mjs
 * Optional: SHOWCASE_URL (default http://app.adi), CHROME_BIN, BROWSER_SCREENSHOTS_DIR,
 * MARKETPLACE_ONLY=1 to check just the catalog (also works against the dashboard).
 * API GETs and the two existing read-only transcript POSTs reach the real backend;
 * every other API write is blocked before it leaves Chrome and fails this check.
 */
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

const origin = new URL(process.env.SHOWCASE_URL ?? "http://app.adi").origin;
const chromePath = process.env.CHROME_BIN ?? (process.platform === "darwin"
  ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : "google-chrome");
const profile = await mkdtemp(resolve(tmpdir(), "adi-primary-smoke-"));
const chrome = Bun.spawn([
  chromePath, "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`,
  "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "about:blank",
], { stdout: "ignore", stderr: "ignore" });

const readPosts = new Set(["/api/agents/run/peek", "/api/agents/run/steps"]);
const errors = [];
const blocked = [];
const interceptionErrors = [];
const requests = [];
const checks = [];
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
const q = (selector) => `document.querySelector(${JSON.stringify(selector)})`;
const grid = 'main[aria-label="Home"] ul[aria-label="Applications"]';
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

async function intercept({ requestId, request }) {
  const url = new URL(request.url);
  const path = url.pathname;
  const nativePath = path.replace(/^\/api\/node\/[a-z0-9-]+\/api(?=\/)/, "/api");
  const label = `${request.method} ${url.origin}${path}`;
  requests.push({ stage, method: request.method, path });
  if (url.origin === origin && (request.method === "GET" || request.method === "HEAD"
    || (request.method === "POST" && readPosts.has(nativePath)))) {
    await send("Fetch.continueRequest", { requestId });
  } else {
    blocked.push(label);
    await send("Fetch.fulfillRequest", {
      requestId, responseCode: 403,
      responseHeaders: [{ name: "Content-Type", value: "application/json" }],
      body: Buffer.from(JSON.stringify({ ok: false, error: "Blocked by the read-only primary app smoke test." })).toString("base64"),
    });
  }
}

async function screenshot(name) {
  if (!process.env.BROWSER_SCREENSHOTS_DIR) return;
  await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
  const shot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
  await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, `primary-app-${name}.png`), Buffer.from(shot.data, "base64"));
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
      // A full document navigation may cancel a read while it is being continued.
      if (!/Invalid InterceptionId|Invalid interceptionId|Invalid requestId|No resource with given identifier|interception.*not found/i.test(error.message)) interceptionErrors.push(error.message);
    });
    let detail;
    if (message.method === "Runtime.exceptionThrown") detail = message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text;
    if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") detail = message.params.args.map((arg) => arg.value ?? arg.description).join(" ");
    if (message.method === "Log.entryAdded" && message.params.entry.level === "error") detail = message.params.entry.text;
    if (detail) errors.push({ stage, detail });
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
    assert.deepEqual(blocked, [], "no unexpected API writes or cross-origin API requests");
    assert.deepEqual(interceptionErrors, [], "API interception remains active");
    assert.deepEqual(errors, [], "zero browser exceptions or console errors");
    checks.push(name);
    console.log(`PASS ${name}`);
  }
  async function navigate(path, selector) {
    await send("Page.navigate", { url: `${origin}${path}` });
    await until(() => evaluate(`!!${q(selector)}`), `${path} renders ${selector}`);
  }
  async function click(selector) {
    await evaluate(`${q(selector)}.scrollIntoView({block:'center'})`);
    const point = await evaluate(`(() => {const r=${q(selector)}.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
    await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, ...point });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, ...point });
  }
  async function tokenColor(selector, property = "backgroundColor") {
    const color = await until(() => evaluate(`(() => {const el=${q(selector)};if(!el)return null;const color=getComputedStyle(el)[${JSON.stringify(property)}];return color.startsWith('oklch(')?color:null;})()`), `${selector} applies a theme token`);
    assert.match(color, /^oklch\(/);
  }
  async function settingsLink() {
    assert.equal(await evaluate(`${q('[data-slot="global-navigation"] a[aria-label="Settings"]')}?.href`), `${origin}/extended/settings`);
  }

  await Promise.all([send("Runtime.enable"), send("Page.enable"), send("Log.enable"), send("Network.enable")]);
  await send("Network.setBypassServiceWorker", { bypass: true });
  await send("Fetch.enable", { patterns: [{ urlPattern: "*/api/*", requestStage: "Request" }] });
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });

  await check("Marketplace opens the new catalog from global navigation", async () => {
    await navigate("/marketplace", 'main[aria-label="Marketplace"]');
    assert.equal(await evaluate("document.title"), "Marketplace — ADI");
    assert.equal(await evaluate(`${q('a[aria-label="Marketplace"]')}.getAttribute('aria-current')`), "page");
    await until(() => evaluate(`document.querySelectorAll('main[aria-label="Marketplace"] [data-slot="card"]').length > 0`), "marketplace packages load");
    const media = await evaluate(`fetch('https://raw.githubusercontent.com/adi-family/marketplace/main/apps/marketplace.json').then(response=>response.json()).then(data=>({icons:data.apps.filter(item=>item.icon).length,previews:data.apps.filter(item=>item.gallery?.length).length,videos:data.apps.reduce((count,item)=>count+(item.gallery?.filter(image=>image.kind==="video").length??0),0),captions:data.apps.reduce((count,item)=>count+(item.gallery?.filter(image=>image.caption).length??0),0),gallery:data.apps.reduce((count,item)=>count+(item.gallery?.filter(image=>image.kind!=="video").length??0),0)}))`);
    assert(media.icons > 0 && media.previews > 0, "live catalog publishes images");
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="marketplace-icon"]').length`), media.icons, "catalog icons render");
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="marketplace-preview"]').length`), media.previews, "catalog previews render");
    async function imagesLoaded() {
      return evaluate(`Promise.all(Array.from(document.querySelectorAll('main[aria-label="Marketplace"] img'), async image => {await image.decode();return image.naturalWidth>0;})).then(results=>results.length>0&&results.every(Boolean))`);
    }
    await until(imagesLoaded, "marketplace icons and previews decode");
    await evaluate(`document.querySelectorAll('main[aria-label="Marketplace"] button[aria-expanded="false"]').forEach(button=>button.click())`);
    await until(() => evaluate(`document.querySelectorAll('[data-slot="marketplace-gallery"] img').length === ${media.gallery}`), "all package gallery images render");
    await until(imagesLoaded, "expanded gallery images decode");
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="marketplace-gallery"] img').length`), media.gallery, "gallery screenshots render");
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="marketplace-gallery"] video[poster][controls]').length`), media.videos, "videos retain posters and playback controls");
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="marketplace-gallery"] figcaption').length`), media.captions, "gallery captions render");
    await screenshot("marketplace-expanded");
    await evaluate(`document.querySelectorAll('main[aria-label="Marketplace"] button[aria-expanded="true"]').forEach(button=>button.click())`);
    await tokenColor('main[aria-label="Marketplace"] [data-slot="card"]');
    if (process.env.MARKETPLACE_ONLY !== "1") await settingsLink();
    await screenshot("marketplace");
  });

  if (process.env.MARKETPLACE_ONLY !== "1") {
    await check("Home renders the live application grid and styled New chat", async () => {
      await navigate("/", grid);
      await until(() => evaluate(`${q(grid)}?.getAttribute('aria-busy')==='false'`), "live Home catalog settles");
      assert.equal(await evaluate("document.title"), "Home — ADI");
      assert.equal(await evaluate(`getComputedStyle(${q(grid)}).display`), "grid");
      assert.equal(await evaluate(`${q('[data-slot="home-new-chat"]')}?.textContent.trim()`), "New chat");
      await tokenColor('[data-slot="home-new-chat"]');
      await settingsLink();
      const health = await evaluate(`fetch('/api/health').then(async response=>({status:response.status,data:await response.json()}))`);
      assert.equal(health.status, 200);
      assert.equal(health.data.ok, true);
      assert.equal(health.data.service, "adi-app");
      assert(requests.some((request) => request.stage === stage && request.path === "/api/dashboards"), "Home reads the native catalog directly");
      assert(requests.some((request) => request.stage === stage && request.path === "/api/fleet/dashboards"), "Home reads the native fleet catalog directly");
      assert.equal(await evaluate(`document.querySelectorAll('main[aria-label="Home"] [role="alert"]').length`), 0, "live catalog has no request errors");
      await screenshot("home");
    });

    await check("trailing-slash library reload renders card and button theme tokens", async () => {
      await navigate("/library/", '#gallery #button-example [data-slot="button"]');
      await until(() => evaluate("location.pathname==='/library'"), "library URL is canonical");
      assert.equal(await evaluate(`${q('#card-example')}?.dataset.slot`), "card");
      await tokenColor('#button-example [data-slot="button"]');
      await tokenColor('#card-example');
      await screenshot("library");
    });

    await check("trailing-slash chat reload renders composer and same-origin native links", async () => {
      // Explicit empty agent keeps this a new draft instead of opening a real transcript.
      await navigate("/interface/?agent=", '[data-slot="app-chat"] [data-slot="chat-composer-input"]');
      await until(() => evaluate(`location.pathname==='/interface' && !!${q('[data-slot="app-chat-sidebar"]')}`), "chat URL is canonical and its sidebar is mounted");
      assert.equal(await evaluate(`${q('[data-slot="chat-composer-input"]')}.value`), "");
      await settingsLink();
      await click('button[aria-label="Conversation sources"]');
      const fleet = '[data-slot="app-chat-menu"] a[role="menuitem"]';
      await until(() => evaluate(`!!${q(fleet)}`), "Fleet link is exposed in the sources menu");
      assert.equal(await evaluate(`${q(fleet)}.href`), `${origin}/extended/settings/fleet`);
      assert.match(await evaluate(`${q(fleet)}.textContent`), /Fleet/);
      await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
      await screenshot("new-chat");
    });

    await check("native Settings still loads the original control panel", async () => {
      await navigate("/extended/settings", '.adi-workbench .adi-main h1.adi-bar__title');
      assert.equal(await evaluate(`${q('.adi-workbench .adi-main h1.adi-bar__title')}.textContent.trim()`), "Settings");
      assert.equal(await evaluate(`Array.from(document.querySelectorAll('.adi-workbench button.adi-linkrow .adi-linkrow__title'), el=>el.textContent.trim()).includes('Fleet')`), true);
      assert.equal(await evaluate(`!!${q('[data-slot="global-navigation"]')}`), false, "native Settings uses its original shell");
      await screenshot("native-settings");
    });

  }
  console.log(`\n${checks.length} primary app smoke checks passed; zero writes, exceptions or console errors.`);
} catch (error) {
  console.error(`Primary app smoke failed during ${stage}; ${checks.length} checks completed.`);
  if (errors.length) console.error(`Browser errors: ${JSON.stringify(errors)}`);
  if (blocked.length) console.error(`Blocked API requests: ${JSON.stringify(blocked)}`);
  if (interceptionErrors.length) console.error(`Interception errors: ${JSON.stringify(interceptionErrors)}`);
  console.error(`API request paths: ${JSON.stringify(requests)}`);
  if (evaluate && socket?.readyState === WebSocket.OPEN) {
    try {
      console.error(JSON.stringify(await evaluate(`({path:location.pathname,title:document.title,home:!!${q(grid)},gallery:!!${q('#gallery')},chat:!!${q('[data-slot="app-chat"]')},native:!!${q('.adi-workbench')}})`)));
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
