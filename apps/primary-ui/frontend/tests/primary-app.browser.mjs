/**
 * Read-only smoke against the activated primary ADI app using an isolated Chrome profile.
 * Run: bun frontend/tests/primary-app.browser.mjs
 * Optional: SHOWCASE_URL (default http://app.adi), CHROME_BIN, BROWSER_SCREENSHOTS_DIR,
 * MARKETPLACE_ONLY=1 to check just the catalog (also works against the dashboard).
 * API GETs and the read-only POSTs (two transcript reads, a trigger's log tail, the event log's
 * stored rows) reach the real backend; every other API write is blocked before it leaves Chrome
 * and fails this check.
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

const readPosts = new Set(["/api/agents/run/peek", "/api/agents/run/steps", "/api/triggers/log", "/api/diagnostics/query"]);
// The rail's event log files each event the page reports with `POST /api/diagnostics/report`,
// which adds a row to the machine's stored log. A read-only smoke must not, so it is acknowledged
// here without leaving Chrome. Listed in the summary like the stubbed requests.
const acknowledgedPosts = new Set(["/api/diagnostics/report"]);
// The native Settings page refreshes the updater's record with `POST /api/update/check` whenever
// that record has gone stale (about half a day). That request leaves the machine and rewrites
// ~/.adi/mono/update/state.json, which a read-only smoke must not do — and which could flip the
// panel into an auto-update mid-check — so it is answered with the current `GET /api/update`
// record instead of being blocked. Listed in the summary so a run shows it happened.
const stubbedPosts = new Set(["/api/update/check"]);
const errors = [];
const blocked = [];
const stubbed = [];
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
  } else if (url.origin === origin && request.method === "POST" && acknowledgedPosts.has(nativePath)) {
    stubbed.push(label);
    await send("Fetch.fulfillRequest", {
      requestId, responseCode: 200,
      responseHeaders: [{ name: "Content-Type", value: "application/json" }],
      body: Buffer.from(JSON.stringify({ ok: true })).toString("base64"),
    });
  } else if (url.origin === origin && request.method === "POST" && stubbedPosts.has(nativePath)) {
    stubbed.push(label);
    const record = await fetch(`${url.origin}${path.replace(/\/check$/, "")}`);
    await send("Fetch.fulfillRequest", {
      requestId, responseCode: record.status,
      responseHeaders: [{ name: "Content-Type", value: record.headers.get("content-type") ?? "application/json" }],
      body: Buffer.from(await record.arrayBuffer()).toString("base64"),
    });
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
    assert.equal(await evaluate(`${q('[data-slot="global-navigation"] a[aria-label="Settings"]')}?.href`), `${origin}/settings`);
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
    const media = await evaluate(`fetch('${process.env.MARKETPLACE_ONLY === "1" ? "/api/app" : "/api"}/marketplace').then(response=>response.json()).then(data=>({icons:data.apps.filter(item=>item.icon).length,previews:data.apps.filter(item=>item.gallery?.length).length}))`);
    // The catalog is operator data: icons are expected of every entry, a gallery is the publisher's choice.
    assert(media.icons > 0, "live catalog publishes icons");
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="marketplace-icon"]').length`), media.icons, "catalog icons render");
    // Cards carry no banner: the gallery is the package page's business.
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="marketplace-preview"]').length`), 0, "catalog cards show no gallery banner");
    const packageLinks = await evaluate(`Array.from(document.querySelectorAll('[data-slot="marketplace-package-link"]'),link=>({href:link.href,tag:link.tagName}))`);
    assert(packageLinks.length > 0 && packageLinks.every(link=>link.tag === "A" && link.href.startsWith(`${origin}/marketplace/`)), "package actions navigate to React detail pages");
    async function imagesLoaded() {
      return evaluate(`Promise.all(Array.from(document.querySelectorAll('main[aria-label="Marketplace"] img'), async image => {await image.decode();return image.naturalWidth>0;})).then(results=>results.length>0&&results.every(Boolean))`);
    }
    await until(imagesLoaded, "marketplace icons and previews decode");
    // Cards are summaries only: no inline preview toggle, galleries live on the package page.
    assert.equal(await evaluate(`document.querySelectorAll('main[aria-label="Marketplace"] [data-slot="card"] button[aria-expanded]').length`), 0, "cards have no quick preview toggle");
    await tokenColor('main[aria-label="Marketplace"] [data-slot="card"]');
    // The topbar alone says whose marketplace this is; the page does not repeat it.
    assert.match(await evaluate(`${q('button[aria-label="Marketplace machine"]')}?.textContent ?? ''`), /This machine/, "the topbar names the machine being browsed");
    assert.equal(await evaluate(`document.querySelectorAll('[data-slot="marketplace-machine"]').length`), 0, "no machine and sources line under the filters");
    if (process.env.MARKETPLACE_ONLY !== "1") await settingsLink();
    await screenshot("marketplace");
  });

  if (process.env.MARKETPLACE_ONLY !== "1") {
    await check("Package page opens the React installation dialog", async () => {
      // The live catalog is operator data, so the package is picked from it rather than named here — a
      // published entry can be withdrawn. The project-name prefill follows the page's own rule: the
      // scaffold element's published name, else the package name.
      const sample = await evaluate(`fetch('/api/marketplace').then(response=>response.json()).then(data=>{const apps=data.apps.filter(app=>app.marketplace==='adi');const app=apps.find(app=>!(app.elements??[]).some(el=>el.kind==='project'))??apps[0];return app?{slug:app.slug,name:app.name,project:(app.elements??[]).find(el=>el.kind==='project')?.name??app.name}:null;})`);
      assert(sample, "the live catalog lists at least one package");
      await navigate("/marketplace", '[data-slot="marketplace-package-link"]');
      await click(`[data-slot="marketplace-package-link"][href="/marketplace/adi/${sample.slug}"]`);
      await until(() => evaluate(`location.pathname === '/marketplace/adi/${sample.slug}' && ${q('[data-slot="marketplace-package"]')}?.textContent.includes(${JSON.stringify(sample.name)})`), `React ${sample.slug} detail page loads`);
      assert(await evaluate(`!!${q('[data-slot="global-navigation"]')}`), 'package keeps the React application shell');
      await send('Page.reload');
      await until(() => evaluate(`!!${q('[data-slot="marketplace-install"]')}`), 'direct reload keeps the React package page');
      await click('[data-slot="marketplace-install"]');
      await until(() => evaluate(`!!${q('[data-slot="marketplace-install-form"] input[aria-label="Project name"]')}`), "existing installation destination form opens");
      assert.equal(await evaluate(`${q('input[aria-label="Project name"]')}.value`), sample.project);
      await screenshot("package-install");
      await click('[data-slot="marketplace-install-form"] button[type="button"]');
      await until(() => evaluate(`!${q('[data-slot="marketplace-install-form"]')}`), "installation dialog closes without installing");
    });

    await check("Installed package opens React copy management", async () => {
      const installed = await evaluate(`fetch('/api/marketplace').then(response=>response.json()).then(data=>data.apps.find(app=>app.marketplace==='adi'&&app.installs?.length))`);
      if (!installed) return;
      await navigate("/marketplace", '[data-slot="marketplace-package-link"]');
      await click(`[data-slot="marketplace-package-link"][href="/marketplace/adi/${installed.slug}"]`);
      await until(() => evaluate(`!!${q('[data-slot="marketplace-installed"]')}`), "installed copy controls render");
      assert((await evaluate(`${q('[data-slot="marketplace-installed"]')}.textContent`)).includes(installed.installs[0].id), "installed copy identity is visible");
      assert(await evaluate(`!!${q('[data-slot="marketplace-installed"] a')} || !!${q('[data-slot="marketplace-installed"] button')}`), "existing Open or Start actions are available");
      await screenshot("package-installed");
    });

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
      assert(await evaluate(`!!${q('[data-slot="app-topbar"] button[aria-label="Conversation sources"]')}`), "the machine selector is in the Home topbar");
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
      const fleet = '[data-slot="node-selector-menu"] a[role="menuitem"]';
      await until(() => evaluate(`!!${q(fleet)}`), "Fleet link is exposed in the sources menu");
      assert.equal(await evaluate(`${q(fleet)}.href`), `${origin}/extended/settings/fleet`);
      assert.match(await evaluate(`${q(fleet)}.textContent`), /Fleet/);
      await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
      await screenshot("new-chat");
    });

    await check("Settings lists this machine's triggers and survives a direct reload", async () => {
      const section = '[data-slot="settings-triggers"]';
      const settled = `${section} [data-slot="trigger-row"], ${section} [data-slot="triggers-empty"]`;
      await navigate("/", grid);
      // Triggers are operator data: the page is compared with what the machine reports, and nothing is written.
      const live = await evaluate(`fetch('/api/triggers').then(response=>response.json()).then(data=>data.triggers.map(trigger=>trigger.name))`);
      await click('[data-slot="global-navigation"] a[aria-label="Settings"]');
      await until(() => evaluate(`location.pathname==='/settings/triggers' && !!${q(settled)}`), "the rail opens Settings on its Triggers section");
      assert.equal(await evaluate("document.title"), "Triggers — Settings — ADI");
      assert.equal(await evaluate(`${q('[data-slot="app-topbar"] h1')}.textContent`), "Settings");
      assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-slot="trigger-row"]'),row=>row.dataset.trigger)`), live, "every trigger this machine defines is listed, in the machine's order");
      assert.equal(await evaluate(`${q('[data-slot="settings-old-ui"]')}.href`), `${origin}/extended/settings`, "the old settings are one click away, on this panel");
      assert.match(await evaluate(`${q('button[aria-label="Settings machine"]')}?.textContent ?? ''`), /This machine/);
      assert.equal(await evaluate(`document.querySelectorAll('main[aria-label="Settings"] [role="alert"]').length`), 0, "no request errors");
      await tokenColor('[data-slot="trigger-new"]');
      // The native app has to hand these addresses to the React shell, or a reload lands in the old panel.
      for (const path of ["/settings", "/settings/", "/settings/triggers/"]) {
        // The page already on screen matches the same selectors, so wait for the new document.
        await evaluate("window.previousDocument=true");
        await send("Page.navigate", { url: `${origin}${path}` });
        await until(() => evaluate(`window.previousDocument===undefined && location.pathname==='/settings/triggers' && !!${q(settled)}`), `a direct load of ${path} renders the React Settings page at its canonical address`);
        assert(await evaluate(`!!${q('[data-slot="global-navigation"]')}`), `${path} keeps the React application shell`);
      }
      await screenshot("settings");
      if (live.length) {
        const name = live[0];
        await navigate(`/settings/triggers?${new URLSearchParams({ trigger: name })}`, '[data-slot="trigger-form"] [name="code"]');
        assert.equal(await evaluate(`${q('[data-slot="trigger-form"] [name="name"]')}.value`), name);
        assert.equal(await evaluate(`${q('[data-slot="trigger-save"]')}.disabled`), true, "an untouched definition has nothing to save");
        await click('nav[aria-label="Trigger views"] a:last-child');
        await until(() => evaluate(`!!${q('[data-slot="trigger-log"] pre')} || !!${q('[data-slot="trigger-log-empty"]')}`), "the trigger's log tail is read");
        assert(requests.some((request) => request.stage === stage && request.method === "POST" && request.path === "/api/triggers/log"), "the log is read from the native API");
        await screenshot("settings-log");
      }
      await navigate("/settings/triggers?new=1", '[data-slot="trigger-form"] [name="name"]');
      assert.equal(await evaluate(`${q('[data-slot="trigger-form"] [name="name"]')}.value`), "");
      assert(await evaluate(`document.querySelectorAll('[data-slot="trigger-form"] [data-preset]').length > 0`), "the machine's presets are offered");
    });

    await check("Secrets lists this machine's secrets by name and asks for no value", async () => {
      // Secrets are operator data: only how many there are is compared, nothing is revealed, and
      // no screenshot is kept — a picture of this page would carry every secret's name.
      const section = '[data-slot="settings-secrets"]';
      const settled = `${section} [data-slot="secret-row"], ${section} [data-slot="secrets-empty"]`;
      // A direct load: the native app hands this address to the React shell.
      await navigate("/settings/secrets", settled);
      assert.equal(await evaluate("document.title"), "Secrets — Settings — ADI");
      assert(await evaluate(`!!${q('[data-slot="global-navigation"]')}`), "Secrets keeps the React application shell");
      assert.equal(await evaluate(`${q('[data-slot="settings-nav-item"][aria-current="page"]')}.textContent.trim()`), "Secrets");
      const live = await evaluate(`fetch('/api/secrets').then(response=>response.json()).then(data=>({total:data.secrets.length,global:data.secrets.filter(secret=>!secret.project).length,leaked:data.secrets.some(secret=>'value' in secret)}))`);
      assert.equal(live.leaked, false, "the list API carries no value");
      assert.equal(await evaluate(`document.querySelectorAll('[data-slot="secret-row"]').length`), live.total, "every secret this machine holds is listed");
      if (live.total) {
        assert.match(await evaluate(`${q('select[aria-label="Scope"]')}.options[0].textContent`), new RegExp(`^All scopes \\(${live.total}\\)$`));
        assert.equal(await evaluate(`document.querySelectorAll('[data-slot="secret-row"][data-scope=""]').length`), live.global, "global secrets are told from project ones");
        assert.equal(await evaluate(`document.querySelectorAll('[data-slot="secret-reveal"]').length`), live.total, "each value can be revealed, one at a time");
      }
      assert.equal(await evaluate(`document.querySelectorAll('[data-slot="secret-value"]').length`), 0, "no value is shown");
      assert.equal(await evaluate(`document.querySelectorAll('main[aria-label="Settings"] [role="alert"]').length`), 0, "no request errors");
      await tokenColor('[data-slot="secret-new"]');
      await navigate("/settings/secrets?new=1", '[data-slot="secret-form"] [name="value"]');
      assert.equal(await evaluate(`${q('[data-slot="secret-form"] [name="value"]')}.dataset.masked`), "true", "a value being typed is masked");
      assert.deepEqual(requests.filter((request) => request.stage === stage && request.method === "POST" && !request.path.endsWith("/diagnostics/query")), [], "listing secrets and opening the form sent nothing and asked for no value");
    });

    await check("native Settings still loads the original control panel", async () => {
      await navigate("/extended/settings", '.adi-workbench .adi-main h1.adi-bar__title');
      assert.equal(await evaluate(`${q('.adi-workbench .adi-main h1.adi-bar__title')}.textContent.trim()`), "Settings");
      assert.equal(await evaluate(`Array.from(document.querySelectorAll('.adi-workbench button.adi-linkrow .adi-linkrow__title'), el=>el.textContent.trim()).includes('Fleet')`), true);
      assert.equal(await evaluate(`!!${q('[data-slot="global-navigation"]')}`), false, "native Settings uses its original shell");
      await screenshot("native-settings");
    });

  }
  console.log(`\n${checks.length} primary app smoke checks passed; zero writes, exceptions or console errors.${stubbed.length ? ` Answered locally instead of forwarding: ${JSON.stringify(stubbed)}` : ""}`);
} catch (error) {
  console.error(`Primary app smoke failed during ${stage}; ${checks.length} checks completed.`);
  if (errors.length) console.error(`Browser errors: ${JSON.stringify(errors)}`);
  if (blocked.length) console.error(`Blocked API requests: ${JSON.stringify(blocked)}`);
  if (stubbed.length) console.error(`Stubbed API requests: ${JSON.stringify(stubbed)}`);
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
