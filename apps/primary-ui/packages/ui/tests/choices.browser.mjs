/**
 * Real-browser checks against the running showcase, without extra dependencies.
 * Run: bun packages/ui/tests/choices.browser.mjs
 * Optional: CHROME_BIN=/path/to/chrome SHOWCASE_URL=http://localhost:5173/library
 * Optional screenshots: BROWSER_SCREENSHOTS_DIR=/tmp
 */
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const chromePath = process.env.CHROME_BIN ?? (process.platform === "darwin"
  ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
  : "google-chrome");
const url = process.env.SHOWCASE_URL ?? "http://design-system.adi/library";
const profile = await mkdtemp(resolve(tmpdir(), "design-system-choice-tests-"));
const fixture = resolve(dirname(fileURLToPath(import.meta.url)), "choices.fixture.tsx");
const chrome = Bun.spawn([
  chromePath, "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`,
  "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "about:blank",
], { stdout: "ignore", stderr: "ignore" });

const errors = [];
const checks = [];
let socket;
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function until(fn, description, timeout = 6000) {
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

try {
  const port = await until(async () => Number((await readFile(resolve(profile, "DevToolsActivePort"), "utf8")).split("\n")[0]), "Chrome debugging port");
  const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  socket = new WebSocket(tabs.find((tab) => tab.type === "page").webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let nextId = 1;
  const pending = new Map();
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id) {
      const waiting = pending.get(message.id);
      if (!waiting) return;
      pending.delete(message.id);
      clearTimeout(waiting.timer);
      message.error ? waiting.reject(new Error(JSON.stringify(message.error))) : waiting.resolve(message.result);
    }
    if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text);
    if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") errors.push(message.params.args.map((item) => item.value ?? item.description).join(" "));
    if (message.method === "Log.entryAdded" && message.params.entry.level === "error") errors.push(message.params.entry.text);
  };
  function send(method, params = {}) {
    const id = nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 10000);
      pending.set(id, { resolve, reject, timer });
      socket.send(JSON.stringify({ id, method, params }));
    });
  }
  async function evaluate(expression) {
    const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    return result.result.value;
  }
  const query = (selector) => `document.querySelector(${JSON.stringify(selector)})`;
  const attr = (selector, name) => evaluate(`${query(selector)}?.getAttribute(${JSON.stringify(name)})`);
  const value = (selector) => evaluate(`${query(selector)}?.value`);
  const text = (selector) => evaluate(`${query(selector)}?.textContent`);
  const focus = (selector) => evaluate(`${query(selector)}.focus()`);
  async function click(selector) {
    await evaluate(`${query(selector)}.scrollIntoView({block:'center'})`);
    const rect = await evaluate(`(() => { const r=${query(selector)}.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
    await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, ...rect });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, ...rect });
  }
  async function key(name, modifiers = 0) {
    const codes = { ArrowDown: 40, ArrowUp: 38, Enter: 13, Escape: 27, Tab: 9, Backspace: 8, Home: 36, End: 35, a: 65 };
    await send("Input.dispatchKeyEvent", { type: "keyDown", key: name, code: name === "a" ? "KeyA" : name, windowsVirtualKeyCode: codes[name], modifiers });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: name, code: name === "a" ? "KeyA" : name, windowsVirtualKeyCode: codes[name], modifiers });
  }
  async function type(selector, contents) {
    await click(selector);
    if (await value(selector)) {
      await evaluate(`${query(selector)}.select()`);
      await key("Backspace");
    }
    if (contents) await send("Input.insertText", { text: contents });
  }
  async function closed(selector) {
    await until(async () => await attr(selector, "aria-expanded") === "false", `closed ${selector}`);
  }
  async function optionsContain(label) {
    return evaluate(`Array.from(document.querySelectorAll('[role="option"]')).some(el => el.textContent.includes(${JSON.stringify(label)}))`);
  }
  async function check(name, fn) {
    await fn();
    checks.push(name);
    console.log(`PASS ${name}`);
  }

  await Promise.all([send("Runtime.enable"), send("Page.enable"), send("Log.enable")]);
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
  await send("Page.navigate", { url });
  await until(() => evaluate(`!!document.querySelector('[data-slot="button"]') && !!document.querySelector('[data-slot="card"]') && !!document.getElementById('autocomplete-local')`), "live gallery renders", 15000);

  await check("gallery renders all seven usage examples with token colors", async () => {
    const ids = ["select-framework", "multi-frameworks", "autocomplete-local", "autocomplete-async", "autocomplete-create", "autocomplete-async-create", "multi-async"];
    for (const id of ids) assert.equal(await evaluate(`!!document.getElementById(${JSON.stringify(id)})`), true, id);
    for (const dark of [false, true]) {
      await evaluate(`document.documentElement.classList.toggle('dark', ${dark})`);
      await until(async () => /^oklch\(/.test(await evaluate(`getComputedStyle(document.querySelector('[data-slot="button"][class*="bg-primary"]')).backgroundColor`)), "theme transition settles");
      const background = await evaluate(`getComputedStyle(document.querySelector('[data-slot="button"][class*="bg-primary"]')).backgroundColor`);
      const selectBackground = await evaluate(`getComputedStyle(document.getElementById('select-framework')).backgroundColor`);
      assert.match(background, /^oklch\(/);
      assert.match(selectBackground, /^oklch\(/);
    }
    await evaluate("document.documentElement.classList.remove('dark')");
    assert.equal(await evaluate("document.querySelectorAll('pre').length >= 7"), true, "usage snippets are visible");
  });

  const sidebar = '[data-slot="app-secondary-navigation"] [data-slot="scroll-sidebar"]';
  const sidebarToggle = `${sidebar} [data-slot="scroll-sidebar-toggle"]`;
  const sidebarLink = (id) => `${sidebar} a[href$="#${id}"]`;
  const sidebarCurrent = () => attr(`${sidebar} a[aria-current="location"]`, "href");
  const galleryUrl = new URL(url);
  galleryUrl.hash = "";
  async function sidebarAt(id) {
    try {
      await until(async () => (await sidebarCurrent())?.endsWith(`#${id}`), `sidebar selects ${id}`);
    } catch (error) {
      const state = await evaluate(`({hash:location.hash,y:scrollY,current:${query(`${sidebar} a[aria-current="location"]`)}?.getAttribute('href'),top:document.getElementById(${JSON.stringify(id)})?.getBoundingClientRect().top})`);
      throw new Error(`${error.message}: ${JSON.stringify(state)}`);
    }
    assert.equal(await evaluate(`document.querySelectorAll('${sidebar} a[aria-current="location"]').length`), 1, "exactly one current section");
  }
  async function clickSidebarLink(id) {
    // Do not scroll the navigation into view: that can move the document and
    // conceal regressions in explicit destinations near the page bottom.
    const rect = await evaluate(`(() => { const r=${query(sidebarLink(id))}.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
    assert.equal(await evaluate(`${rect.y} >= 0 && ${rect.y} <= window.innerHeight`), true, `sidebar link ${id} is visible`);
    await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, ...rect });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, ...rect });
    await until(() => evaluate(`location.hash === ${JSON.stringify(`#${id}`)}`), `section ${id} updates the URL`);
    await sidebarAt(id);
    await until(() => evaluate(`document.activeElement?.id === ${JSON.stringify(id)}`), `section ${id} receives focus`);
    // Let the router's fragment-restoration frame finish before testing a new
    // user scroll or navigation.
    await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  }
  async function naturalScrollTo(id) {
    await evaluate(`(() => { const section=document.getElementById(${JSON.stringify(id)}); window.scrollTo({top:window.scrollY+section.getBoundingClientRect().top-(parseFloat(getComputedStyle(section).scrollMarginTop)||0),behavior:'instant'}); })()`);
    await sidebarAt(id);
  }
  async function sidebarScreenshot(mode) {
    if (!process.env.BROWSER_SCREENSHOTS_DIR) return;
    const path = resolve(process.env.BROWSER_SCREENSHOTS_DIR, `design-system-sidebar-${mode}.png`);
    const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
    await writeFile(path, Buffer.from(screenshot.data, "base64"));
    console.log(`Screenshot: ${path}`);
  }

  await check("scroll sidebar follows normal scrolling in both directions", async () => {
    await until(() => evaluate(`!!${query(sidebar)} && !!document.getElementById('scroll-sidebar-example')`), "shared scroll sidebar renders");
    const controlledIds = await evaluate("Array.from(document.querySelectorAll('[data-slot=scroll-sidebar-toggle]'), toggle => toggle.getAttribute('aria-controls'))");
    assert.ok(controlledIds.length >= 2, "live gallery renders multiple reusable sidebar instances");
    assert.equal(new Set(controlledIds).size, controlledIds.length, "every sidebar has a unique disclosure target");
    for (const id of controlledIds) assert.equal(await evaluate(`document.getElementById(${JSON.stringify(id)})?.tagName`), "NAV");
    await naturalScrollTo("button-example");
    await naturalScrollTo("app-layout-example");
    assert.equal(await evaluate("window.location.hash"), "", "reading sections does not rewrite the URL");
    await until(async () => /^oklch\(/.test(await evaluate(`getComputedStyle(${query(`${sidebar} a[aria-current="location"]`)}).backgroundColor`)), "active sidebar color transition settles");
    assert.match(await evaluate(`getComputedStyle(${query(`${sidebar} a[aria-current="location"]`)}).backgroundColor`), /^oklch\(/, "active section uses theme tokens");
    await sidebarScreenshot("desktop");
  });

  await check("scroll sidebar preserves short destinations until the reader scrolls", async () => {
    await clickSidebarLink("theme-toggle-example");
    assert.equal(await evaluate("window.scrollY+window.innerHeight >= document.documentElement.scrollHeight-2"), true, "penultimate short section is clamped at the bottom");
    await pause(100);
    await sidebarAt("theme-toggle-example");
    await clickSidebarLink("card-example");
    await pause(100);
    await sidebarAt("card-example");
    await naturalScrollTo("button-example");
    const readingPosition = await evaluate("window.scrollY");
    await pause(350);
    await sidebarAt("button-example");
    assert.equal(await evaluate("window.scrollY"), readingPosition, "a deferred router restoration does not interrupt reading");
    assert.equal(await evaluate("window.location.hash"), "#card-example", "scroll tracking does not replace the selected fragment");
    await clickSidebarLink("card-example");
    assert.equal(await evaluate("window.scrollY+window.innerHeight >= document.documentElement.scrollHeight-2"), true, "selecting the same fragment again still scrolls to it");
  });

  await check("scroll sidebar supports native anchors without a router adapter", async () => {
    await naturalScrollTo("scroll-sidebar-example");
    const nativeLink = '#scroll-sidebar-example [data-slot="scroll-sidebar"] a[href="#button-example"]';
    await click(nativeLink);
    await sidebarAt("button-example");
    await until(() => evaluate("document.activeElement?.id === 'button-example'"), "native anchor focuses its target");
    assert.equal(await evaluate("location.hash"), "#button-example");
  });

  await check("scroll sidebar restores direct fragments and browser history", async () => {
    await send("Page.navigate", { url: "about:blank" });
    await until(() => evaluate("location.href === 'about:blank'"), "fresh direct-link navigation starts");
    await send("Page.navigate", { url: `${galleryUrl}#card-example` });
    await until(() => evaluate(`location.hash === '#card-example' && !!${query(sidebar)} && !!document.getElementById('card-example')`), "direct fragment renders", 15000);
    await sidebarAt("card-example");
    await send("Page.reload", { ignoreCache: true });
    await until(() => evaluate(`performance.getEntriesByType('navigation')[0]?.type === 'reload' && !!${query(sidebar)} && !!document.getElementById('card-example')`), "fragment reload renders", 15000);
    await sidebarAt("card-example");
    assert.equal(await evaluate("window.scrollY > 0"), true, "direct fragment scrolls to its section");
    // Native anchors create history entries independently of the router's
    // deferred location renders, which can coalesce two rapid Link clicks.
    for (const id of ["button-example", "card-example"]) {
      await click(`#scroll-sidebar-example [data-slot="scroll-sidebar"] a[href="#${id}"]`);
      await until(() => evaluate(`location.hash === ${JSON.stringify(`#${id}`)}`), `native navigation to ${id}`);
      await sidebarAt(id);
    }
    const history = await send("Page.getNavigationHistory");
    assert.ok(history.entries[history.currentIndex - 1].url.endsWith("#button-example"), "Back has a distinct Button history entry");
    await send("Page.navigateToHistoryEntry", { entryId: history.entries[history.currentIndex - 1].id });
    await until(() => evaluate("window.location.hash === '#button-example'"), "history goes back to Button");
    await sidebarAt("button-example");
    await send("Page.navigateToHistoryEntry", { entryId: history.entries[history.currentIndex].id });
    await until(() => evaluate("window.location.hash === '#card-example'"), "history goes forward to Card");
    await sidebarAt("card-example");
  });

  await check("scroll sidebar routes from another page and focuses its destination", async () => {
    await click('#chat-thread-example [data-kind="agent"] [data-slot="chat-tool-call-link"]');
    await until(() => evaluate("window.location.pathname === '/library/agents/layout-reviewer' && !document.getElementById('button-example')"), "agent detail route renders");
    assert.equal(await sidebarCurrent(), undefined, "another page has no current gallery section");
    await clickSidebarLink("button-example");
    assert.equal(await evaluate("window.location.pathname"), "/library");
  });

  await check("mobile scroll sidebar exposes accessible disclosure and destination focus", async () => {
    await send("Emulation.setDeviceMetricsOverride", { width: 390, height: 1000, deviceScaleFactor: 1, mobile: true });
    await send("Page.navigate", { url: galleryUrl.toString() });
    await until(() => evaluate(`window.location.hash === '' && !!${query(sidebarToggle)} && !!document.getElementById('button-example')`), "mobile gallery renders", 15000);
    await evaluate("window.scrollTo({top:0,behavior:'instant'})");
    await closed(sidebarToggle);
    const navigationId = await attr(sidebarToggle, "aria-controls");
    assert.equal(await evaluate(`document.querySelectorAll('[id='+CSS.escape(${JSON.stringify(navigationId)})+']').length`), 1, "disclosure controls one navigation landmark");
    assert.equal(await evaluate(`getComputedStyle(document.getElementById(${JSON.stringify(navigationId)})).display`), "none");
    await click(sidebarToggle);
    await until(async () => await attr(sidebarToggle, "aria-expanded") === "true", "mobile navigation expands");
    await sidebarScreenshot("mobile");
    await focus(sidebarLink("button-example"));
    await key("Escape");
    await closed(sidebarToggle);
    assert.equal(await evaluate(`document.activeElement === ${query(sidebarToggle)}`), true, "Escape returns focus to the disclosure");
    await click(sidebarToggle);
    await clickSidebarLink("button-example");
    await closed(sidebarToggle);
    assert.equal(await evaluate(`getComputedStyle(document.getElementById(${JSON.stringify(navigationId)})).display`), "none", "choosing a section collapses the mobile menu");
    const position = await evaluate("(() => { const section=document.getElementById('button-example'); return {top:section.getBoundingClientRect().top,margin:parseFloat(getComputedStyle(section).scrollMarginTop)}; })()");
    assert.ok(Math.abs(position.top-position.margin) <= 1, "section offset is measured after mobile collapse");
    assert.equal(await evaluate("document.documentElement.scrollWidth <= window.innerWidth"), true, "mobile sidebar has no page overflow");
    await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
    await send("Page.navigate", { url: galleryUrl.toString() });
    await until(() => evaluate("window.location.hash === '' && !!document.getElementById('autocomplete-local')"), "desktop gallery restored", 15000);
  });

  if (process.env.BROWSER_SCREENSHOTS_DIR) {
    for (const mode of ["light", "dark", "mobile"]) {
      await evaluate(`document.documentElement.classList.toggle('dark', ${mode === "dark"})`);
      await send("Emulation.setDeviceMetricsOverride", { width: mode === "mobile" ? 390 : 1280, height: 1000, deviceScaleFactor: 1, mobile: mode === "mobile" });
      await click("#autocomplete-local");
      await pause(250);
      const path = resolve(process.env.BROWSER_SCREENSHOTS_DIR, `design-system-choice-${mode}.png`);
      const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
      await writeFile(path, Buffer.from(screenshot.data, "base64"));
      console.log(`Screenshot: ${path}`);
      assert.equal(await evaluate("document.documentElement.scrollWidth <= window.innerWidth"), true, `${mode} has no page overflow`);
      await key("Escape");
    }
    await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
    await evaluate("document.documentElement.classList.remove('dark')");
  }

  await evaluate(`import(${JSON.stringify(`/@fs${fixture}`)}).then(module => module.mount())`);
  await until(() => evaluate("!!document.getElementById('test-local')"), "fixture renders");

  await check("native Select preserves disabled options and reports selection", async () => {
    assert.equal(await evaluate("document.querySelector('#test-select option[value=beta]').disabled"), true);
    // CDP cannot consistently drive the operating system's native select popup on macOS.
    await evaluate("document.getElementById('test-select').value = 'gamma'; document.getElementById('test-select').dispatchEvent(new Event('change', {bubbles:true}))");
    await until(async () => await text("#native-value") === "gamma", "native change callback");
    assert.equal(await value("#test-select"), "gamma");
  });

  await check("local autocomplete supports arrows, Enter, Escape, clear and keyword filtering", async () => {
    await click("#test-local");
    await until(() => optionsContain("Alpha"), "local options");
    await key("ArrowDown");
    await key("Enter");
    await until(async () => (await text("#single-value")).includes('"alpha"'), "local selection");
    await closed("#test-local");
    await click('[data-slot="autocomplete"]:has(#test-local) button[aria-label="Clear selection"]');
    await until(async () => await text("#single-value") === "null", "cleared selection");
    await click("#test-local");
    await key("ArrowDown");
    await key("ArrowDown");
    assert.equal(await evaluate("document.getElementById(document.getElementById('test-local').getAttribute('aria-activedescendant')).textContent"), "Gamma", "keyboard skips disabled Beta");
    await type("#test-local", "third");
    await until(() => optionsContain("Gamma"), "keyword match");
    assert.equal(await optionsContain("Alpha"), false);
    await key("Escape");
    await closed("#test-local");
    assert.equal(await evaluate("document.activeElement.id"), "test-local");
  });

  await check("multi select adds, removes and preserves keyboard focus", async () => {
    await click("#test-multi");
    await key("ArrowDown");
    await key("Enter");
    await until(async () => (await text("#multi-value")).includes('"alpha"'), "multi first choice");
    await type("#test-multi", "Gamma");
    await key("ArrowDown");
    await key("Enter");
    await until(async () => (await text("#multi-value")).includes('"gamma"'), "multi second choice");
    assert.equal(JSON.parse(await text("#multi-value")).length, 2);
    await click('button[aria-label="Remove Alpha"]');
    await until(async () => !((await text("#multi-value")).includes('"alpha"')), "remove chip");
    await focus("#test-multi");
    await key("Escape");
    await key("Backspace");
    await until(async () => await text("#multi-value") === "[]", "Backspace removes last chip");
  });

  await check("Tab and outside click dismiss without stealing focus", async () => {
    await click("#test-local");
    await key("Tab");
    await closed("#test-local");
    assert.notEqual(await evaluate("document.activeElement.id"), "test-local");
    await click("#test-local");
    await click("#test-outside");
    await closed("#test-local");
    assert.equal(await evaluate("document.activeElement.id"), "test-outside");
  });

  await check("remote options display loading and ignore stale responses", async () => {
    await type("#test-async", "old");
    await until(() => evaluate("window.__choiceTest.requests.includes('old')"), "old request started");
    assert.equal(await evaluate("document.querySelector('[data-slot=choice-content]').textContent.toLowerCase().includes('loading')"), true);
    await type("#test-async", "new");
    await until(() => optionsContain("NEW result"), "new response");
    await pause(400);
    assert.equal(await optionsContain("OLD result"), false);
    assert.equal(await optionsContain("NEW result"), true);
    assert.equal(await evaluate("window.__choiceTest.aborted.includes('old')"), true);
    await key("ArrowDown");
    await key("Enter");
    await closed("#test-async");
    assert.equal(await value("#test-async"), "NEW result");
    await click("#test-async");
    assert.equal(await attr("#test-async", "aria-busy"), "true", "reopening refreshes results");
    assert.equal(await optionsContain("NEW result"), false, "reopening does not expose stale options");
  });

  await check("remote empty, error and Retry states recover", async () => {
    await type("#test-async", "empty");
    await until(() => evaluate("document.querySelector('[data-slot=choice-content]')?.textContent.toLowerCase().includes('no results')"), "empty state");
    await type("#test-async", "retry");
    await until(() => evaluate("Array.from(document.querySelectorAll('button')).some(el => el.textContent === 'Retry')"), "retry button");
    await key("Enter");
    await until(() => optionsContain("RETRY result"), "retry success");
    await key("Escape");
  });

  await check("async MultiSelect selects remote options", async () => {
    await type("#test-async-multi", "new");
    await until(() => optionsContain("NEW result"), "remote multi result");
    await key("ArrowDown");
    await key("Enter");
    await until(() => evaluate("!!document.querySelector('[data-slot=multi-select]:has(#test-async-multi) button[aria-label=\"Remove NEW result\"]')"), "remote chip");
    await key("Escape");
  });

  await check("creation is optional, deduplicated, asynchronous and recoverable", async () => {
    await type("#test-local", "novel");
    assert.equal(await evaluate("!!document.querySelector('[data-slot=choice-create]')"), false);
    await key("Escape");
    await type("#test-create", "Alpha");
    assert.equal(await evaluate("!!document.querySelector('[data-slot=choice-create]')"), false, "existing labels cannot be created twice");
    await type("#test-create", "Novel");
    await until(() => evaluate("!!document.querySelector('[data-slot=choice-create]')"), "create option");
    await key("ArrowDown");
    await key("Enter");
    await until(async () => (await text("#created-value")).includes('"created-novel"'), "created server option");
    await type("#test-create", "reject");
    await key("ArrowDown");
    await key("Enter");
    await until(() => evaluate("document.querySelector('[data-slot=choice-content]')?.textContent.includes('Could not create this option')"), "creation error");
    await type("#test-create", "Recovered");
    await key("ArrowDown");
    await key("Enter");
    await until(async () => (await text("#created-value")).includes('"created-recovered"'), "creation recovers");
  });

  await check("async creation waits for results and default creation works", async () => {
    await type("#test-async-create", "empty");
    await until(() => evaluate("!!document.querySelector('[data-slot=choice-create]')"), "remote create option");
    await key("ArrowDown");
    await key("Enter");
    await until(async () => await value("#test-async-create") === "empty", "remote created selection");
    await closed("#test-async-create");
    await type("#test-default-create", "Fresh");
    await key("ArrowDown");
    await key("Enter");
    await until(() => evaluate("!!document.querySelector('button[aria-label=\"Remove Fresh\"]')"), "default-created chip");
    await key("Escape");
  });

  await check("disabled controls stay disabled and do not open", async () => {
    for (const selector of ["#test-disabled-select", "#test-disabled-local", "#test-disabled-multi"]) {
      assert.equal(await evaluate(`${query(selector)}.disabled`), true);
      await click(selector);
      assert.notEqual(await attr(selector, "aria-expanded"), "true");
    }
  });

  await check("Backspace skips a locked trailing chip", async () => {
    await focus("#test-locked");
    await key("Backspace");
    await until(() => evaluate("!document.querySelector('[data-slot=multi-select]:has(#test-locked) button[aria-label=\"Remove Alpha\"]')"), "last removable chip removed");
    assert.equal(await evaluate("document.querySelector('[data-slot=multi-select]:has(#test-locked) button[aria-label=\"Remove Beta\"]').disabled"), true);
  });

  await check("authoritative option updates override the created cache", async () => {
    await type("#test-refresh", "Fresh");
    await key("ArrowDown");
    await key("Enter");
    await until(() => evaluate("!!document.querySelector('[data-slot=multi-select]:has(#test-refresh) button[aria-label=\"Remove Fresh\"]')"), "cached created choice");
    await key("Escape");
    await click("#test-refresh-options");
    await click("#test-refresh");
    await until(() => optionsContain("Updated"), "refreshed option label");
    assert.equal(await optionsContain("Fresh"), false, "cached label does not override refresh");
    assert.equal(await evaluate("Array.from(document.querySelectorAll('[role=option]')).find(el => el.textContent === 'Updated').getAttribute('aria-disabled')"), "true");
    assert.equal(await evaluate("document.querySelector('[data-slot=multi-select]:has(#test-refresh) button[aria-label=\"Remove Fresh\"]').disabled"), true, "refreshed disabled state locks selected chip");
    await key("Escape");
  });

  await check("pending creation cannot satisfy a required form", async () => {
    assert.equal(await evaluate("document.getElementById('test-required-form').checkValidity()"), false);
    await type("#test-required", "Pending");
    await key("ArrowDown");
    await key("Enter");
    await until(async () => await attr("#test-required", "aria-busy") === "true", "creation pending");
    assert.equal(await evaluate("document.getElementById('test-required-form').checkValidity()"), false, "pending create remains invalid");
    await closed("#test-required");
    assert.equal(await evaluate("document.getElementById('test-required-form').checkValidity()"), true);
    assert.equal(await value("#test-required"), "Pending");
  });

  await check("form values and native reset preserve defaults", async () => {
    const formValues = () => evaluate("Array.from(new FormData(document.getElementById('test-form')).entries())");
    assert.deepEqual(await formValues(), [["native", "alpha"], ["single", "alpha"], ["many", "alpha"]]);
    await type("#test-form-single", "Gamma");
    await key("ArrowDown");
    await key("Enter");
    await until(async () => (await formValues()).some(([name, val]) => name === "single" && val === "gamma"), "form selection updated");
    await click("#test-reset");
    await until(async () => await value("#test-form-single") === "Alpha", "form reset updates displayed label");
    assert.deepEqual(await formValues(), [["native", "alpha"], ["single", "alpha"], ["many", "alpha"]]);
  });

  assert.deepEqual(errors, [], "browser exceptions and console errors");
  console.log(`\n${checks.length} browser scenarios passed; no exceptions or console errors.`);
} finally {
  socket?.close();
  chrome.kill();
  await chrome.exited;
  await rm(profile, { recursive: true, force: true });
}
