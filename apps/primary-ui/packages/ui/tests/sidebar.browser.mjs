/** Live desktop sidebar drag, keyboard, reset, and mobile checks.
 * Run: bun packages/ui/tests/sidebar.browser.mjs
 */
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

const chromePath = process.env.CHROME_BIN ?? (process.platform === "darwin"
  ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
  : "google-chrome");
const url = process.env.SHOWCASE_URL ?? "http://design-system.adi/library";
const profile = await mkdtemp(resolve(tmpdir(), "design-system-chat-tests-"));
const chrome = Bun.spawn([
  chromePath, "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`,
  "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "about:blank",
], { stdout: "ignore", stderr: "ignore" });

const errors = [];
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
  await send("Runtime.enable");
  await send("Log.enable");
  await send("Page.enable");
  await send("Emulation.setDeviceMetricsOverride", {width:1280,height:900,deviceScaleFactor:1,mobile:false});
  const handle = '[data-slot="app-sidebar-resize-handle"]';
  const sidebar = '[data-slot="app-secondary-navigation"]';
  const width = () => evaluate(`document.querySelector('${sidebar}').getBoundingClientRect().width`);
  for (const route of ["/library", "/interface"]) {
    await send("Page.navigate", {url: new URL(route,url).href});
    await until(() => evaluate(`!!document.querySelector('${handle}')`), "sidebar rendered");
    await pause(300);
    assert.equal(await evaluate(`document.querySelector('[data-slot="app-topbar"]').getBoundingClientRect().height`), 44);
    assert.equal(await evaluate(`document.querySelector('[data-slot="app-sidebar"]').getBoundingClientRect().width`), 48);
    const initial = await width();
    assert.equal(initial, 200);
    const rect = await evaluate(`(() => {const r=document.querySelector('${handle}').getBoundingClientRect();return {x:r.x+4,y:r.y+80}})()`);
    await send("Input.dispatchMouseEvent", {type:"mouseMoved",...rect});
    await send("Input.dispatchMouseEvent", {type:"mousePressed",...rect,button:"left",clickCount:1});
    await send("Input.dispatchMouseEvent", {type:"mouseMoved",x:rect.x+100,y:rect.y,button:"left",buttons:1});
    await send("Input.dispatchMouseEvent", {type:"mouseReleased",x:rect.x+100,y:rect.y,button:"left",clickCount:1});
    await until(async () => await width() === initial+100, "drag grows sidebar");
    await send("Page.reload");
    await until(() => evaluate(`!!document.querySelector('${handle}')`), "sidebar restored after reload");
    await until(async () => await width() === initial+100, "dragged width persists after reload");
    await evaluate(`document.querySelector('${handle}').focus()`);
    for (const [key,expected] of [["ArrowLeft",initial+84],["Home",160],["End",480]]) {
      await send("Input.dispatchKeyEvent",{type:"keyDown",key});
      await send("Input.dispatchKeyEvent",{type:"keyUp",key});
      await until(async () => await width() === expected, key);
    }
    await send("Page.reload");
    await until(() => evaluate(`!!document.querySelector('${handle}')`), "keyboard width restored");
    await until(async () => await width() === 480, "keyboard width persists after reload");
    await evaluate(`document.querySelector('${handle}').dispatchEvent(new MouseEvent('dblclick',{bubbles:true}))`);
    await until(async () => await width() === initial, "reset");
    assert.equal(await evaluate(`localStorage.getItem('adi.sidebar-width.v1')`), null, "reset clears saved width");
    await send("Page.reload");
    await until(() => evaluate(`!!document.querySelector('${handle}')`), "reset reload");
    await until(async () => await width() === initial, "reset persists");
    const color = await evaluate(`getComputedStyle(document.querySelector('[data-slot="app-content"]')).backgroundColor`);
    if(route === "/library") {
      assert.match(color,/oklch/);
      assert.ok(await evaluate(`!!document.querySelector('[data-slot="button"]') && !!document.querySelector('[data-slot="card"]')`));
    }
    await send("Emulation.setDeviceMetricsOverride", {width:390,height:844,deviceScaleFactor:1,mobile:false});
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('${handle}')).display`),"none");
    await send("Emulation.setDeviceMetricsOverride", {width:1280,height:900,deviceScaleFactor:1,mobile:false});
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('[data-slot="app-workspace-controls"]')).position`), 'absolute');
    assert.equal(await evaluate(`document.querySelector('[data-slot="workspace-expand"]').closest('${sidebar}')`), null);
    await evaluate(`document.querySelector('[aria-label="Expand workspace"]').click()`);
    await until(() => evaluate(`getComputedStyle(document.querySelector('[data-slot="app-topbar"]')).display==='none'`), "topbar hidden");
    const contentBounds = () => evaluate(`(() => {const r=document.querySelector('[data-slot="app-content"]').getBoundingClientRect(); return [r.x,r.y,r.width,r.height]})()`);
    assert.deepEqual(await contentBounds(), [initial,0,1280-initial,900]);
    assert.equal(await width(), initial);
    assert.equal(await evaluate(`document.querySelector('${sidebar}').hidden`), false);
    assert.ok(await evaluate(`(() => {const b=document.querySelector('[data-slot="workspace-expand"]').getBoundingClientRect();const s=document.querySelector('${sidebar}').getBoundingClientRect();return b.x >= s.x && b.x < s.x+32 && b.y >= s.y && b.y < s.y+32})()`));
    await send("Emulation.setDeviceMetricsOverride", {width:390,height:844,deviceScaleFactor:1,mobile:false});
    const mobileBounds = await contentBounds();
    assert.equal(mobileBounds[0], 0);
    assert.equal(mobileBounds[2], 390);
    assert.ok(mobileBounds[1] > 0 && mobileBounds[1] < 844);
    assert.equal(mobileBounds[1] + mobileBounds[3], 844);
    await send("Emulation.setDeviceMetricsOverride", {width:1280,height:900,deviceScaleFactor:1,mobile:false});
    const expandedScreenshot = await send("Page.captureScreenshot", {format:"png"});
    await writeFile(`/tmp/design-system-expanded-${route.slice(1)}.png`,Buffer.from(expandedScreenshot.data,"base64"));
    await evaluate(`document.querySelector('[aria-label="Restore navigation"]').click()`);
    await until(async () => await width() === initial, "navigation width restored");
    console.log(`Passed ${route}: drag, keyboard, bounds, reload persistence, reset, mobile`);
  }
  const screenshot = await send("Page.captureScreenshot",{format:"png"});
  await writeFile("/tmp/design-system-sidebar.png",Buffer.from(screenshot.data,"base64"));
  assert.deepEqual(errors,[]);
} finally {
  socket?.close();
  chrome.kill();
  await chrome.exited;
  await rm(profile,{recursive:true,force:true});
}
