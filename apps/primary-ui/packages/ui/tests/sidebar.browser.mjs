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
    const initial = await width();
    assert.equal(initial, route === "/interface" ? 224 : 208);
    const rect = await evaluate(`(() => {const r=document.querySelector('${handle}').getBoundingClientRect();return {x:r.x+4,y:r.y+80}})()`);
    await send("Input.dispatchMouseEvent", {type:"mouseMoved",...rect});
    await send("Input.dispatchMouseEvent", {type:"mousePressed",...rect,button:"left",clickCount:1});
    await send("Input.dispatchMouseEvent", {type:"mouseMoved",x:rect.x+100,y:rect.y,button:"left",buttons:1});
    await send("Input.dispatchMouseEvent", {type:"mouseReleased",x:rect.x+100,y:rect.y,button:"left",clickCount:1});
    await until(async () => await width() === initial+100, "drag grows sidebar");
    await evaluate(`document.querySelector('${handle}').focus()`);
    for (const [key,expected] of [["ArrowLeft",initial+84],["Home",160],["End",480]]) {
      await send("Input.dispatchKeyEvent",{type:"keyDown",key});
      await send("Input.dispatchKeyEvent",{type:"keyUp",key});
      await until(async () => await width() === expected, key);
    }
    await evaluate(`document.querySelector('${handle}').dispatchEvent(new MouseEvent('dblclick',{bubbles:true}))`);
    await until(async () => await width() === initial, "reset");
    const color = await evaluate(`getComputedStyle(document.querySelector('[data-slot="app-content"]')).backgroundColor`);
    if(route === "/library") {
      assert.match(color,/oklch/);
      assert.ok(await evaluate(`!!document.querySelector('[data-slot="button"]') && !!document.querySelector('[data-slot="card"]')`));
    }
    await send("Emulation.setDeviceMetricsOverride", {width:390,height:844,deviceScaleFactor:1,mobile:false});
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('${handle}')).display`),"none");
    await send("Emulation.setDeviceMetricsOverride", {width:1280,height:900,deviceScaleFactor:1,mobile:false});
    console.log(`Passed ${route}: drag, keyboard, bounds, reset, mobile`);
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
