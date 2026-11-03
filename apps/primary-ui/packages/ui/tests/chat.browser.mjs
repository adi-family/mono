/**
 * Real-browser chat checks against the running showcase, without extra dependencies.
 * Run: bun packages/ui/tests/chat.browser.mjs
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
const profile = await mkdtemp(resolve(tmpdir(), "design-system-chat-tests-"));
const fixture = resolve(dirname(fileURLToPath(import.meta.url)), "chat.fixture.tsx");
const chrome = Bun.spawn([
  chromePath, "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`,
  "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "about:blank",
], { stdout: "ignore", stderr: "ignore" });

const errors = [];
const checks = [];
let socket;
let collectFailureDetails;
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
  collectFailureDetails = async () => {
    const page = await evaluate(`(() => ({
      url:location.href,
      title:document.title,
      readyState:document.readyState,
      rootText:document.getElementById('root')?.textContent?.slice(0,1000),
      bodyText:document.body?.textContent?.slice(0,1000),
      viteError:document.querySelector('vite-error-overlay')?.shadowRoot?.textContent?.slice(0,1800),
      slots:Object.fromEntries(['button','card','chat-composer','chat-thread','chat-tool-call'].map(slot=>[slot,document.querySelectorAll('[data-slot="'+slot+'"]').length])),
    }))()`);
    console.error(`Browser page diagnostics: ${JSON.stringify(page, null, 2)}`);
    if (process.env.BROWSER_SCREENSHOTS_DIR) {
      const path = resolve(process.env.BROWSER_SCREENSHOTS_DIR, "design-system-chat-failure.png");
      const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
      await writeFile(path, Buffer.from(screenshot.data, "base64"));
      console.error(`Failure screenshot: ${path}`);
    }
  };
  const query = (selector) => `document.querySelector(${JSON.stringify(selector)})`;
  const attr = (selector, name) => evaluate(`${query(selector)}?.getAttribute(${JSON.stringify(name)})`);
  const value = (selector) => evaluate(`${query(selector)}?.value`);
  const focus = (selector) => evaluate(`${query(selector)}.focus()`);
  async function click(selector) {
    await evaluate(`${query(selector)}.scrollIntoView({block:'center'})`);
    const rect = await evaluate(`(() => { const r=${query(selector)}.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
    await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, ...rect });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, ...rect });
  }
  async function key(name, modifiers = 0) {
    const codes = { Enter: 13, Tab: 9, Backspace: 8, " ": 32 };
    const code = name === " " ? "Space" : name;
    await send("Input.dispatchKeyEvent", { type: "keyDown", key: name, code, windowsVirtualKeyCode: codes[name], modifiers, ...(name === "Enter" ? { text: "\r" } : name === " " ? { text: " " } : {}) });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: name, code, windowsVirtualKeyCode: codes[name], modifiers });
  }
  async function type(contents) {
    await click("#test-draft");
    await evaluate("document.getElementById('test-draft').select()");
    await key("Backspace");
    if (contents) await send("Input.insertText", { text: contents });
  }
  async function check(name, fn) {
    await fn();
    checks.push(name);
    console.log(`PASS ${name}`);
  }
  const submitted = () => evaluate("window.__chatTest.sent");
  const sendButton = '#test-composer button[aria-label="Send message"]';
  const threadScroll = (id) => `document.querySelector('#'+CSS.escape(${JSON.stringify(id)})+' [data-slot="chat-thread-viewport"]')`;
  const threadPosition = (id) => evaluate(`(() => { const el=${threadScroll(id)}; return {top:el.scrollTop,bottom:el.scrollHeight-el.clientHeight-el.scrollTop,height:el.scrollHeight}; })()`);
  const threadMessages = (id) => evaluate(`Array.from(document.querySelectorAll('#'+CSS.escape(${JSON.stringify(id)})+' [data-message-number]'), el=>({number:Number(el.dataset.messageNumber),top:el.getBoundingClientRect().top}))`);
  async function assertThreadOrder(id, reversed) {
    const messages = await threadMessages(id);
    assert.ok(messages.length > 1, "thread contains multiple messages");
    const numbers = messages.map((message) => message.number);
    assert.deepEqual(numbers, [...numbers].sort((a, b) => reversed ? b - a : a - b), "DOM follows the selected chronological order");
    assert.ok(messages.every((message, index) => index === 0 || message.top > messages[index - 1].top), "visual reading order matches DOM reading order");
  }
  const visibleAnchor = (id) => evaluate(`(() => {
    const viewport=${threadScroll(id)}, top=viewport.getBoundingClientRect().top;
    const messages=Array.from(viewport.querySelectorAll('[data-message-number]'));
    const message=messages.find(el=>el.getBoundingClientRect().top >= top && el.getBoundingClientRect().bottom <= top+viewport.clientHeight)
      ?? messages.find(el=>el.getBoundingClientRect().bottom > top);
    return {number:Number(message.dataset.messageNumber),offset:message.getBoundingClientRect().top-top};
  })()`);
  const anchorOffset = (id, number) => evaluate(`(() => { const viewport=${threadScroll(id)}; return viewport.querySelector('[data-message-number="${number}"]').getBoundingClientRect().top-viewport.getBoundingClientRect().top; })()`);
  async function assertAnchorPreserved(id, anchor, message) {
    const actual = await evaluate(`(() => {
      const viewport=${threadScroll(id)}, bounds=viewport.getBoundingClientRect();
      const message=viewport.querySelector('[data-message-number="${anchor.number}"]');
      return {
        number:${anchor.number}, expectedOffset:${anchor.offset},actualOffset:message.getBoundingClientRect().top-bounds.top,
        scrollTop:viewport.scrollTop,scrollHeight:viewport.scrollHeight,clientHeight:viewport.clientHeight,
        viewportTop:bounds.top,pageScrollY:window.scrollY,growingHeight:document.getElementById('test-growing-content').getBoundingClientRect().height,
      };
    })()`);
    assert.ok(Math.abs(actual.actualOffset - anchor.offset) <= 2, `${message}: ${JSON.stringify(actual)}`);
  }
  async function readHistory(id) {
    await evaluate(`(() => { const el=${threadScroll(id)}; el.scrollTop=(el.scrollHeight-el.clientHeight)/2; el.dispatchEvent(new Event('scroll')); })()`);
    await until(() => evaluate(`!!document.querySelector('#'+CSS.escape(${JSON.stringify(id)})+' [data-slot=chat-thread-jump]')`), "history jump control appears");
  }

  await Promise.all([send("Runtime.enable"), send("Page.enable"), send("Log.enable")]);
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
  await send("Page.navigate", { url });
  await until(() => evaluate("!!document.querySelector('[data-slot=button]') && !!document.querySelector('[data-slot=card]') && !!document.querySelector('[data-slot=chat-composer]') && !!document.querySelector('[data-slot=chat-thread]')"), "live gallery renders chat", 15000);

  await check("gallery renders chat examples using theme tokens", async () => {
    for (const kind of ["terminal", "git", "edit", "agent"]) {
      assert.ok(await evaluate(`document.querySelectorAll('#chat-tools-example [data-slot=chat-tool-call][data-kind=${kind}]').length > 0`), `gallery demonstrates ${kind} tools`);
    }
    assert.ok(await evaluate("document.querySelectorAll('#chat-thread-example [data-slot=chat-tool-call][data-kind=agent]').length > 0"), "conversation demonstrates delegated agent work");
    assert.equal(await evaluate("document.querySelector('#chat-tools-example [data-demo=grouped-tool-errors] [data-slot=chat-tool-call-error-count]').textContent"), "1 error", "gallery demonstrates a focused error count");
    assert.equal(await evaluate("document.querySelectorAll('#chat-tools-example [data-demo=grouped-tool-errors] [data-slot=chat-tool-call]').length"), 8, "the compact group contains eight calls");
    assert.equal(await attr('#chat-tools-example [data-demo=grouped-tool-errors] [data-slot="chat-tool-call-trigger"]', "aria-expanded"), "false", "the group starts compact");
    assert.equal(await evaluate("document.querySelectorAll('#chat-thread-example [data-slot=chat-message-avatar], #chat-messages-example [data-slot=chat-message-avatar]').length"), 0, "chat examples omit avatars");
    assert.equal(await evaluate("document.querySelectorAll('#chat-thread-example button[aria-label=\"Reset conversation\"]').length"), 0, "conversation omits its header reset action");
    assert.equal(await evaluate("document.getElementById('chat-thread-example').textContent.includes('A space to think together')"), false, "conversation omits the internal header");
    for (const dark of [false, true]) {
      await evaluate(`document.documentElement.classList.toggle('dark', ${dark})`);
      await until(() => evaluate("/^oklch\\(/.test(getComputedStyle(document.querySelector('#chat-composer-example [data-slot=chat-composer]')).backgroundColor)"), "composer theme color settles");
      await until(() => evaluate("/^oklch\\(/.test(getComputedStyle(document.querySelector('[data-slot=button][class*=bg-primary]')).backgroundColor)"), "button theme transition settles");
      assert.match(await evaluate("getComputedStyle(document.querySelector('[data-slot=button][class*=bg-primary]')).backgroundColor"), /^oklch\(/);
    }
    await evaluate("document.documentElement.classList.remove('dark')");
  });

  await check("agent activity opens its conversation and returns to the design chat", async () => {
    const link = '#chat-thread-example [data-kind="agent"] [data-slot="chat-tool-call-link"]';
    assert.equal(await attr(link, "href"), "/library/agents/layout-reviewer", "agent activity has a real conversation destination");
    for (const section of ["chat-thread-example", "chat-tools-example"]) {
      const text = await evaluate(`document.querySelector('#'+${JSON.stringify(section)}+' [data-kind=agent]').textContent`);
      assert.equal(text.includes("Started by"), false, "agent rows omit parent metadata");
      assert.equal(text.includes("Review complete"), false, "agent rows omit redundant completion metadata");
    }
    await click(link);
    await until(() => evaluate("location.pathname==='/library/agents/layout-reviewer' && !!document.querySelector('[data-demo=agent-conversation] [data-slot=chat-thread]')"), "agent conversation route renders");
    assert.equal(await evaluate("document.getElementById('agent-chat-heading').textContent"), "Layout reviewer");
    assert.equal(await evaluate("document.querySelector('[data-demo=agent-conversation] [data-slot=chat-thread-viewport]').getAttribute('aria-label')"), "Layout reviewer conversation");
    assert.equal(await evaluate("document.querySelectorAll('[data-demo=agent-conversation] [data-slot=chat-message]').length"), 2, "destination shows the delegated task and agent reply");
    assert.ok(await evaluate("document.querySelectorAll('[data-demo=agent-conversation] [data-slot=chat-message-content]')[1].textContent.trim().length > 0"), "agent reply is readable");
    if (process.env.BROWSER_SCREENSHOTS_DIR) {
      for (const mode of ["light", "dark", "mobile"]) {
        await evaluate(`document.documentElement.classList.toggle('dark', ${mode === "dark"})`);
        await send("Emulation.setDeviceMetricsOverride", { width: mode === "mobile" ? 390 : 1280, height: 1000, deviceScaleFactor: 1, mobile: mode === "mobile" });
        await evaluate("document.getElementById('agent-chat-heading').scrollIntoView({block:'start'})");
        await pause(250);
        assert.ok(await evaluate("document.querySelector('[data-demo=agent-conversation] [data-slot=chat-thread-viewport]').getBoundingClientRect().height>=200"), `${mode} agent conversation has readable height`);
        assert.equal(await evaluate("document.documentElement.scrollWidth <= window.innerWidth"), true, `${mode} agent conversation has no page overflow`);
        const path = resolve(process.env.BROWSER_SCREENSHOTS_DIR, `design-system-agent-chat-${mode}.png`);
        const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
        await writeFile(path, Buffer.from(screenshot.data, "base64"));
        console.log(`Screenshot: ${path}`);
      }
      await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
      await evaluate("document.documentElement.classList.remove('dark')");
    }
    await click('[data-slot="agent-chat-back"]');
    await until(() => evaluate("location.pathname==='/library' && !!document.querySelector('#chat-thread-example [data-slot=chat-composer]')"), "back link returns to the design conversation");
    assert.equal(await value('#chat-thread-example [data-slot="chat-composer-input"]'), "", "return starts with a clean draft before interaction checks");
  });

  await check("gallery defaults to top-to-bottom and previews both chronological directions", async () => {
    const select = "#chat-direction";
    const thread = '#chat-thread-example [data-slot="chat-thread"]';
    const composer = '#chat-thread-example [data-slot="chat-composer"]';
    const input = `${composer} [data-slot="chat-composer-input"]`;
    const messages = '#chat-thread-example [data-slot="chat-message"]';
    async function assertComposerPlacement(direction) {
      const placement = await evaluate(`(() => {
        const thread=${query(thread)}, composer=${query(composer)};
        const threadBounds=thread.getBoundingClientRect(), composerBounds=composer.getBoundingClientRect();
        return {
          composerPrecedesThread:!!(composer.compareDocumentPosition(thread)&Node.DOCUMENT_POSITION_FOLLOWING),
          composerTop:composerBounds.top,composerBottom:composerBounds.bottom,
          threadTop:threadBounds.top,threadBottom:threadBounds.bottom,
        };
      })()`);
      const reversed = direction === "bottom-to-top";
      assert.equal(placement.composerPrecedesThread, reversed, "composer DOM order follows the selected direction");
      assert.ok(reversed ? placement.composerBottom <= placement.threadTop : placement.threadBottom <= placement.composerTop, "composer appears beside the newest messages in the selected direction");
    }
    assert.equal(await value(select), "top-to-bottom");
    assert.equal(await attr(thread, "data-direction"), "top-to-bottom");
    await assertComposerPlacement("top-to-bottom");
    const original = await evaluate(`Array.from(document.querySelectorAll(${JSON.stringify(messages)}), el=>el.textContent)`);
    assert.ok(original.length > 1);
    await evaluate(`void(window.__galleryChatInput=${query(input)})`);
    await click(input);
    await send("Input.insertText", { text: "Keep this draft while changing direction" });
    for (const direction of ["bottom-to-top", "top-to-bottom"]) {
      await evaluate(`(() => { const select=${query(select)}; select.value=${JSON.stringify(direction)}; select.dispatchEvent(new Event('change',{bubbles:true})); })()`);
      await until(async () => await attr(thread, "data-direction") === direction, "gallery direction changes");
      await assertComposerPlacement(direction);
      assert.equal(await value(input), "Keep this draft while changing direction", "changing direction preserves the draft");
      assert.equal(await evaluate(`window.__galleryChatInput===${query(input)}`), true, "changing direction preserves the textarea DOM node");
      assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll(${JSON.stringify(messages)}), el=>el.textContent)`), direction === "bottom-to-top" ? [...original].reverse() : original);
      const positions = await evaluate(`Array.from(document.querySelectorAll(${JSON.stringify(messages)}), el=>el.getBoundingClientRect().top)`);
      assert.ok(positions.every((top, index) => index === 0 || top > positions[index - 1]), "gallery visual order matches DOM order");
      assert.equal(await evaluate("document.querySelectorAll('#chat-thread-example [data-slot=chat-message-avatar]').length"), 0);
    }
    await click(input);
    await evaluate(`${query(input)}.select()`);
    await key("Backspace");
    assert.equal(await value(input), "");
    await evaluate("delete window.__galleryChatInput");
  });

  await check("live conversation streams, stops safely and handles another response", async () => {
    const composer = '#chat-thread-example [data-slot="chat-composer"]';
    const input = `${composer} [data-slot="chat-composer-input"]`;
    const messages = '#chat-thread-example [data-slot="chat-message"]';
    const originalCount = await evaluate(`document.querySelectorAll(${JSON.stringify(messages)}).length`);
    await click(input);
    await send("Input.insertText", { text: "What should we build next?" });
    await click(`${composer} button[aria-label="Send message"]`);
    await until(() => evaluate("Array.from(document.querySelectorAll('#chat-thread-example [data-from=assistant] [data-slot=chat-message-content]')).at(-1)?.textContent.includes('Let’s make')"), "live response starts streaming");
    await click(input);
    await send("Input.insertText", { text: "Follow-up draft" });
    await click(`${composer} button[aria-label="Stop response"]`);
    await until(() => evaluate(`!!${query(`${composer} button[aria-label="Send message"]`)}`), "live response stops");
    assert.equal(await value(input), "Follow-up draft", "stopping preserves the next draft");
    assert.equal(await evaluate(`document.querySelectorAll(${JSON.stringify(messages)}).length`), originalCount + 2, "Stop does not send a duplicate");
    const partial = await evaluate("Array.from(document.querySelectorAll('#chat-thread-example [data-from=assistant] [data-slot=chat-message-content]')).at(-1).textContent");
    // Stop in the mutation microtask that exposes the control, before streaming timers run.
    await evaluate(`new Promise((resolve,reject) => {
      const composer=${query(composer)};
      const timeout=setTimeout(()=>{observer.disconnect();reject(new Error('Stop control did not appear'));},2000);
      const observer=new MutationObserver(stopWhenReady);
      function stopWhenReady() {
        const stop=composer.querySelector('button[aria-label="Stop response"]');
        if(!stop) return;
        observer.disconnect();clearTimeout(timeout);stop.click();resolve();
      }
      observer.observe(composer,{childList:true,subtree:true});
      composer.requestSubmit();
      stopWhenReady();
    })`);
    await until(() => evaluate(`!!${query(`${composer} button[aria-label="Send message"]`)}`), "second response stops immediately");
    assert.equal(await evaluate(`document.querySelectorAll(${JSON.stringify(messages)}).length`), originalCount + 4);
    assert.equal(await evaluate("document.querySelectorAll('#chat-thread-example [data-slot=chat-typing-indicator]').length"), 0, "stopped replies do not retain a typing indicator");
    const responseTexts = await evaluate("Array.from(document.querySelectorAll('#chat-thread-example [data-from=assistant] [data-slot=chat-message-content]'), el=>el.textContent)");
    assert.equal(responseTexts.at(-2), partial, "another send does not revive a stopped response");
    assert.equal(responseTexts.at(-1), "Response stopped.", "stopping before the first chunk leaves an explicit response");
    await click(input);
    await send("Input.insertText", { text: "One more question" });
    const streaming = await evaluate(`new Promise((resolve,reject) => {
      const timeout=setTimeout(()=>{observer.disconnect();reject(new Error('No typing indicator appeared for the new response'));},2000);
      const observer=new MutationObserver(()=>{
        const typing=document.querySelectorAll('#chat-thread-example [data-slot=chat-typing-indicator]').length;
        if(!typing) return;
        clearTimeout(timeout);observer.disconnect();
        resolve({typing,previous:Array.from(document.querySelectorAll('#chat-thread-example [data-from=assistant] [data-slot=chat-message-content]')).at(-2).textContent});
      });
      observer.observe(document.getElementById('chat-thread-example'),{childList:true,subtree:true});
      ${query(composer)}.requestSubmit();
    })`);
    assert.equal(streaming.typing, 1, "only the current response shows a typing indicator");
    assert.equal(streaming.previous, "Response stopped.", "a new response leaves an earlier stopped response intact");
    await click(`${composer} button[aria-label="Stop response"]`);
    await send("Page.navigate", { url });
    await until(async () => await evaluate(`document.querySelectorAll(${JSON.stringify(messages)}).length`) === originalCount && await value(input) === "", "gallery reload restores the initial conversation");
    assert.equal(await value(input), "");
  });

  await check("mobile tool summaries preserve readable title space", async () => {
    await send("Emulation.setDeviceMetricsOverride", { width: 390, height: 1000, deviceScaleFactor: 1, mobile: true });
    const titles = await evaluate("Array.from(document.querySelectorAll('#chat-tools-example [data-slot=chat-tool-call-title]')).filter(title=>title.getClientRects().length).map(title=>({text:title.textContent,width:title.getBoundingClientRect().width,content:title.scrollWidth,available:title.closest('[data-slot=chat-tool-call]').getBoundingClientRect().width}))");
    assert.ok(titles.length > 0, "tool gallery contains summaries");
    for (const title of titles) {
      assert.ok(Math.ceil(title.width) >= Math.min(80, title.available, title.content), `mobile tool title has readable width: ${JSON.stringify(title)}`);
    }
    assert.equal(await evaluate("document.documentElement.scrollWidth <= window.innerWidth"), true, "mobile tool gallery has no page overflow");
    await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
  });

  if (process.env.BROWSER_SCREENSHOTS_DIR) {
    for (const mode of ["light", "dark", "mobile"]) {
      await evaluate(`document.documentElement.classList.toggle('dark', ${mode === "dark"})`);
      await send("Emulation.setDeviceMetricsOverride", { width: mode === "mobile" ? 390 : 1280, height: 1000, deviceScaleFactor: 1, mobile: mode === "mobile" });
      await evaluate("document.getElementById('chat-thread-example').scrollIntoView({block:'start'})");
      await pause(250);
      for (const direction of ["top-to-bottom", "bottom-to-top"]) {
        await evaluate(`(() => { const select=document.getElementById('chat-direction'); select.value=${JSON.stringify(direction)}; select.dispatchEvent(new Event('change',{bubbles:true})); })()`);
        await until(async () => await attr('#chat-thread-example [data-slot="chat-thread"]', "data-direction") === direction, "screenshot direction updates");
        await pause(100);
        const path = resolve(process.env.BROWSER_SCREENSHOTS_DIR, `design-system-chat-gallery-${direction}-${mode}.png`);
        const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
        await writeFile(path, Buffer.from(screenshot.data, "base64"));
        console.log(`Screenshot: ${path}`);
      }
      await evaluate("document.getElementById('chat-tools-example').scrollIntoView({block:'start'})");
      await pause(250);
      const toolsPath = resolve(process.env.BROWSER_SCREENSHOTS_DIR, `design-system-chat-tools-${mode}.png`);
      const toolsScreenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
      await writeFile(toolsPath, Buffer.from(toolsScreenshot.data, "base64"));
      console.log(`Screenshot: ${toolsPath}`);
      assert.equal(await evaluate("document.documentElement.scrollWidth <= window.innerWidth"), true, `${mode} gallery has no page overflow`);
    }
    await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
    await evaluate("document.documentElement.classList.remove('dark')");
  }

  await evaluate(`import(${JSON.stringify(`/@fs${fixture}`)}).then(module => module.mount())`);
  await until(() => evaluate("!!document.getElementById('test-draft')"), "fixture renders");

  await check("tool disclosures expose linked content and support Enter and Space", async () => {
    const trigger = '#test-tool-disclosure [data-slot="chat-tool-call-trigger"]';
    const contentId = await attr(trigger, "aria-controls");
    assert.ok(contentId, "disclosure names its controlled content");
    assert.equal(await evaluate(`document.getElementById(${JSON.stringify(contentId)}).closest('[data-slot=chat-tool-call]').id`), "test-tool-disclosure");
    assert.equal(await attr(trigger, "aria-expanded"), "false");
    assert.equal(await evaluate(`document.getElementById(${JSON.stringify(contentId)}).hidden`), true);
    assert.equal(await attr(trigger, "type"), "button", "disclosure does not submit an ancestor form");
    await focus(trigger);
    await key("Enter");
    await until(async () => await attr(trigger, "aria-expanded") === "true", "Enter opens the tool");
    assert.equal(await evaluate(`document.getElementById(${JSON.stringify(contentId)}).hidden`), false);
    await evaluate("void(window.__toolOutputNode=document.getElementById('test-tool-output'))");
    await key(" ");
    await until(async () => await attr(trigger, "aria-expanded") === "false", "Space closes the tool");
    assert.equal(await evaluate(`document.activeElement===${query(trigger)}`), true, "keyboard focus remains on the disclosure");
    await key(" ");
    await until(async () => await attr(trigger, "aria-expanded") === "true", "Space reopens the tool");
    assert.equal(await evaluate("document.getElementById('test-tool-output')===window.__toolOutputNode"), true, "disclosure preserves output DOM state");
    const contentIds = await evaluate("Array.from(document.querySelectorAll('#test-tool-calls [aria-controls]'), el=>el.getAttribute('aria-controls'))");
    assert.equal(new Set(contentIds).size, contentIds.length, "each tool controls a distinct content region");
    assert.equal(await evaluate("document.querySelectorAll('#test-tool-summary button, #test-tool-summary [aria-expanded]').length"), 0, "a summary with no details has no empty disclosure");
  });

  await check("controlled tools wait for host updates and keep disclosure across status changes", async () => {
    const trigger = '#test-tool-controlled [data-slot="chat-tool-call-trigger"]';
    await click(trigger);
    await until(() => evaluate("window.__chatTest.toolOpenRequests.length===1"), "controlled disclosure notifies host");
    assert.deepEqual(await evaluate("window.__chatTest.toolOpenRequests"), [true]);
    assert.equal(await attr(trigger, "aria-expanded"), "false", "controlled value remains authoritative");
    await evaluate("window.__chatTest.setToolOpen(true)");
    await until(async () => await attr(trigger, "aria-expanded") === "true", "host opens disclosure");
    const labels = { running: "Running", completed: "Completed", error: "Failed", cancelled: "Cancelled", pending: "Pending" };
    for (const [status, label] of Object.entries(labels)) {
      await evaluate(`window.__chatTest.setToolStatus(${JSON.stringify(status)})`);
      await until(async () => await attr("#test-tool-controlled", "data-status") === status, "tool lifecycle updates");
      assert.equal(await evaluate("document.querySelector('#test-tool-controlled [data-slot=chat-tool-call-status]').textContent.trim()"), label);
      assert.equal(await attr(trigger, "aria-expanded"), "true", `${status} preserves the controlled disclosure`);
      assert.equal(await attr('#test-tool-disclosure [data-slot="chat-tool-call-trigger"]', "aria-expanded"), "true", `${status} preserves the uncontrolled disclosure`);
    }
    await focus(trigger);
    await key("Enter");
    await until(() => evaluate("window.__chatTest.toolOpenRequests.length===2"), "controlled close notifies host");
    assert.deepEqual(await evaluate("window.__chatTest.toolOpenRequests"), [true, false]);
    assert.equal(await attr(trigger, "aria-expanded"), "true", "close request waits for host state");
    await evaluate("window.__chatTest.setToolOpen(false)");
    await until(async () => await attr(trigger, "aria-expanded") === "false", "host closes disclosure");
  });

  await check("grouped tool errors expose counts and retain keyboard disclosure", async () => {
    const trigger = '#test-tool-group [data-slot="chat-tool-call-trigger"]';
    const badge = (id) => evaluate(`document.querySelector('#'+${JSON.stringify(id)}+' [data-slot=chat-tool-call-error-count]')?.textContent ?? null`);
    assert.equal(await attr("#test-tool-group", "data-grouped"), "true");
    assert.equal(await badge("test-tool-group"), "1 error");
    assert.equal(await badge("test-tool-group-plural"), "3 errors");
    assert.equal(await badge("test-tool-group-unknown"), "Error", "an unknown count still identifies an error");
    assert.equal(await badge("test-tool-group-zero"), null, "zero errors have no warning badge");
    assert.equal(await badge("test-tool-group-complete"), null, "healthy groups with no count have no warning badge");
    assert.equal(await attr(trigger, "aria-expanded"), "false");
    const contentId = await attr(trigger, "aria-controls");
    assert.equal(await evaluate(`document.getElementById(${JSON.stringify(contentId)}).hidden`), true);
    await focus(trigger);
    await key("Enter");
    await until(async () => await attr(trigger, "aria-expanded") === "true", "Enter opens grouped calls");
    assert.equal(await evaluate(`document.getElementById(${JSON.stringify(contentId)}).hidden`), false);
    assert.equal(await attr("#test-tool-group-failure", "data-status"), "error");
    await key(" ");
    await until(async () => await attr(trigger, "aria-expanded") === "false", "Space closes grouped calls");
    assert.equal(await evaluate(`document.activeElement===${query(trigger)}`), true, "group disclosure retains keyboard focus");
    await key("Enter");
    await until(async () => await attr(trigger, "aria-expanded") === "true", "group can reopen for individual call inspection");
  });

  await check("only the group error count and failed child use error color in both themes", async () => {
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 0, y: 0 });
    for (const dark of [false, true]) {
      await evaluate(`document.documentElement.classList.toggle('dark', ${dark})`);
      const colors = await evaluate(`(() => {
        const color=selector=>getComputedStyle(document.querySelector(selector)).color;
        return {
          title:color('#test-tool-group [data-slot=chat-tool-call-title]'),
          icon:color('#test-tool-group [data-slot=chat-tool-call-trigger] svg'),
          chevron:color('#test-tool-group [data-slot=chat-tool-call-trigger] svg:last-of-type'),
          badge:color('#test-tool-group [data-slot=chat-tool-call-error-count]'),
          healthy:color('#test-tool-group-zero [data-slot=chat-tool-call-title]'),
          childSuccess:color('#test-tool-group-success [data-slot=chat-tool-call-title]'),
          childFailure:color('#test-tool-group-failure [data-slot=chat-tool-call-title]'),
          childFailureIcon:color('#test-tool-group-failure svg'),
        };
      })()`);
      const mode = dark ? "dark" : "light";
      assert.equal(colors.title, colors.healthy, `${mode} group title retains the normal muted color`);
      assert.equal(colors.icon, colors.healthy, `${mode} group icon stays neutral`);
      assert.equal(colors.chevron, colors.healthy, `${mode} group disclosure stays neutral`);
      assert.notEqual(colors.badge, colors.title, `${mode} only the count receives error emphasis`);
      assert.equal(colors.childFailure, colors.badge, `${mode} failed child title remains red`);
      assert.equal(colors.childFailureIcon, colors.badge, `${mode} failed child icon remains red`);
      assert.equal(colors.childSuccess, colors.healthy, `${mode} successful child remains neutral`);
    }
    await evaluate("document.documentElement.classList.remove('dark')");
  });

  await check("tool command, output and diff preserve literal text and accurate change counts", async () => {
    assert.equal(await evaluate("document.querySelector('#test-tool-disclosure [data-slot=chat-tool-call-title]').textContent"), await evaluate("window.__chatTest.command"));
    assert.equal(await evaluate("document.getElementById('test-tool-output').textContent"), await evaluate("window.__chatTest.output"));
    assert.equal(await attr("#test-tool-output", "aria-label"), "Verification output");
    assert.equal(await attr("#test-tool-output", "tabindex"), "0", "long output is keyboard scrollable");
    assert.equal(await evaluate("document.querySelectorAll('#test-tool-calls script').length"), 0, "tool strings do not become markup");
    assert.equal(await evaluate("window.__toolMarkupExecuted===undefined"), true);
    assert.deepEqual(await evaluate("Array.from(document.querySelectorAll('#test-tool-diff [data-slot=chat-tool-diff-line]'), el=>({type:el.dataset.type,content:el.querySelector('code').textContent}))"), await evaluate("window.__chatTest.diffLines.map(({type,content})=>({type,content}))"));
    assert.equal(await evaluate("document.querySelector('#test-tool-diff [data-slot=chat-tool-diff-header] [aria-label]').getAttribute('aria-label')"), "2 additions, 1 deletions");
    assert.equal(await evaluate("document.querySelector('#test-tool-diff [data-slot=chat-tool-diff-viewport]').getAttribute('role')"), "region");
    assert.match(await attr('#test-tool-diff [data-slot="chat-tool-diff-viewport"]', "aria-label"), /^Changes to packages\/ui\/src\/components\//);
    const lineNumbers = await evaluate("Array.from(document.querySelectorAll('#test-tool-diff [data-slot=chat-tool-diff-line]'), el=>Array.from(el.children).slice(0,2).map(number=>number.textContent))");
    assert.deepEqual(lineNumbers, [["9", "9"], ["10", ""], ["", "10"], ["", "11"], ["11", "12"]], "added and removed lines retain the correct old/new line numbers");
  });

  await check("agent activity uses the compact disclosure and preserves its task through status updates", async () => {
    const row = "#test-tool-agent";
    const trigger = `${row} [data-slot="chat-tool-call-trigger"]`;
    assert.equal(await attr(row, "data-kind"), "agent");
    assert.equal(await evaluate(`!!${query(trigger)}.querySelector('.lucide-bot')`), true, "completed agent activity has an agent icon");
    assert.equal(await attr(trigger, "aria-expanded"), "false", "agent details start collapsed");
    await click(trigger);
    await until(async () => await attr(trigger, "aria-expanded") === "true", "agent details open");
    assert.equal(await evaluate("document.getElementById('test-agent-result').textContent"), await evaluate("window.__chatTest.agentResult"), "agent results remain literal prose");
    assert.equal(await evaluate(`!!${query(row)}.querySelector('pre, code, mobile')`), false, "agent details need no terminal output renderer");
    await evaluate("void(window.__agentResultNode=document.getElementById('test-agent-result'))");
    for (const status of ["running", "completed"]) {
      await evaluate(`window.__chatTest.setAgentStatus(${JSON.stringify(status)})`);
      await until(async () => await attr(row, "data-status") === status, "host updates agent status");
      assert.equal(await attr(trigger, "aria-expanded"), "true", `${status} preserves agent disclosure`);
      assert.equal(await evaluate(`!!${query(trigger)}.querySelector(${JSON.stringify(status === "running" ? ".lucide-loader-circle" : ".lucide-bot")})`), true, `${status} shows the appropriate activity icon`);
      assert.equal(await evaluate(`document.querySelector('#test-tool-agent [data-slot=chat-tool-call-title]').textContent`), await evaluate("window.__chatTest.agentTitle"), `${status} preserves the host's agent name and task`);
      assert.equal(await evaluate("document.getElementById('test-agent-result')===window.__agentResultNode"), true, `${status} preserves agent detail content`);
    }
  });

  await check("linked agent activity navigates natively and keeps details on a separate control", async () => {
    const row = "#test-tool-agent-link";
    const link = `${row} [data-slot="chat-tool-call-link"]`;
    const trigger = `${row} [data-slot="chat-tool-call-trigger"]`;
    const destination = "#agent-chat-destination";
    assert.equal(await evaluate(`${query(link)}.tagName`), "A", "the title uses native link semantics");
    assert.equal(await attr(link, "href"), destination);
    assert.equal(await attr(link, "title"), "Open the layout reviewer conversation", "native anchor props are forwarded");
    assert.equal(await attr(link, "aria-expanded"), null, "the navigation link has no disclosure state");
    assert.equal(await attr(trigger, "type"), "button");
    assert.equal(await attr(trigger, "aria-label"), "Details for Layout reviewer · Check chat layout");
    assert.equal(await attr(trigger, "aria-expanded"), "false");
    assert.equal(await evaluate(`${query(link)}.parentElement===${query(trigger)}.parentElement`), true, "navigation and disclosure are sibling controls");
    assert.equal(await evaluate(`${query(row)}.querySelectorAll('a button, button a').length`), 0, "interactive controls are never nested");
    assert.equal(await evaluate(`${query(link)}.querySelector('[data-slot=chat-tool-call-title]').textContent`), await evaluate("window.__chatTest.agentTitle"));
    assert.equal(await evaluate(`!!${query(link)}.querySelector('.lucide-bot')`), true, "the agent icon belongs to the link");
    const contentId = await attr(trigger, "aria-controls");
    assert.ok(contentId, "the separate disclosure identifies its content");
    assert.equal(await evaluate(`document.getElementById(${JSON.stringify(contentId)}).hidden`), true);
    await focus(link);
    await key("Enter");
    await until(() => evaluate(`location.hash===${JSON.stringify(destination)}`), "Enter follows the agent conversation link");
    assert.equal(await evaluate("window.__chatTest.agentLinkActivations"), 1, "the host's link handler runs without suppressing native navigation");
    assert.equal(await evaluate("document.getElementById('test-agent-chat-heading').textContent"), "Layout reviewer conversation");
    assert.equal(await attr(trigger, "aria-expanded"), "false", "following the conversation does not expand activity details");
    await focus(trigger);
    await key("Enter");
    await until(async () => await attr(trigger, "aria-expanded") === "true", "the separate control opens activity details");
    assert.equal(await evaluate("location.hash"), destination, "opening details does not navigate");
    assert.equal(await evaluate("document.getElementById('test-agent-link-result').textContent"), await evaluate("window.__chatTest.agentResult"));
    await evaluate(`void(window.__agentLinkNode=${query(link)})`);
    for (const status of ["running", "completed"]) {
      await evaluate(`window.__chatTest.setAgentStatus(${JSON.stringify(status)})`);
      await until(async () => await attr(row, "data-status") === status, "linked agent status updates");
      assert.equal(await attr(link, "href"), destination, `${status} preserves the agent conversation target`);
      assert.equal(await evaluate(`${query(link)}===window.__agentLinkNode`), true, `${status} preserves the navigation link`);
      assert.equal(await attr(trigger, "aria-expanded"), "true", `${status} preserves the independent details disclosure`);
      assert.equal(await evaluate(`!!${query(link)}.querySelector(${JSON.stringify(status === "running" ? ".lucide-loader-circle" : ".lucide-bot")})`), true, `${status} updates the icon inside the link`);
    }
    await focus(trigger);
    await key(" ");
    await until(async () => await attr(trigger, "aria-expanded") === "false", "Space closes linked activity details");
    assert.equal(await evaluate("window.__chatTest.agentLinkActivations"), 1, "details interactions never activate the navigation link");
  });

  await check("empty and whitespace drafts cannot be sent", async () => {
    assert.equal(await attr("#test-draft", "aria-label"), "Message");
    assert.equal(await attr("#test-draft", "name"), "message");
    assert.equal(await evaluate(`${query(sendButton)}.disabled`), true);
    await type("  \n  ");
    await key("Enter");
    assert.deepEqual(await submitted(), []);
    assert.equal(await evaluate(`${query(sendButton)}.disabled`), true);
  });

  await check("Enter trims and sends once, preserves pending draft and clears on success", async () => {
    await type("  Hello chat  ");
    await key("Enter");
    await until(async () => (await submitted()).length === 1, "send starts");
    assert.deepEqual(await submitted(), ["Hello chat"]);
    assert.equal(await value("#test-draft"), "  Hello chat  ");
    await key("Enter");
    await evaluate("document.getElementById('test-composer').requestSubmit()");
    assert.equal((await submitted()).length, 1, "pending submission is single-flight");
    await evaluate("window.__chatTest.resolve()");
    await until(async () => await value("#test-draft") === "", "successful submission clears draft");
  });

  await check("Shift+Enter adds a newline and host handlers may prevent Enter", async () => {
    await type("First line");
    await key("Enter", 8);
    assert.equal(await value("#test-draft"), "First line\n");
    assert.equal((await submitted()).length, 1);
    await evaluate("window.__chatTest.preventEnter = true");
    await key("Enter");
    assert.equal((await submitted()).length, 1);
    assert.equal(await value("#test-draft"), "First line\n");
    assert.ok(await evaluate("window.__chatTest.keydowns > 0"), "textareaProps keyboard handler is forwarded");
    await evaluate("window.__chatTest.preventEnter = false");
  });

  await check("IME composition and legacy composition keys do not send", async () => {
    await type("Composing");
    await evaluate("document.getElementById('test-draft').dispatchEvent(new KeyboardEvent('keydown', {key:'Enter',code:'Enter',bubbles:true,isComposing:true}))");
    await evaluate("document.getElementById('test-draft').dispatchEvent(new KeyboardEvent('keydown', {key:'Enter',code:'Enter',bubbles:true,keyCode:229}))");
    assert.equal((await submitted()).length, 1);
  });

  await check("typing during an asynchronous send survives success", async () => {
    await type("Send this");
    await click(sendButton);
    await until(async () => (await submitted()).length === 2, "second send starts");
    assert.equal(await evaluate("document.activeElement?.id"), "test-draft", "clicking Send restores input focus");
    await type("Next draft");
    await evaluate("window.__chatTest.resolve()");
    await until(async () => await evaluate(`${query(sendButton)}.disabled`) === false, "pending send finishes");
    assert.equal(await value("#test-draft"), "Next draft");
  });

  await check("failed sends preserve the draft, announce an error and can be retried", async () => {
    await click(sendButton);
    await until(async () => (await submitted()).length === 3, "failing send starts");
    await evaluate("window.__chatTest.reject()");
    await until(() => evaluate("!!document.querySelector('#test-composer [role=alert]')"), "failure alert appears");
    assert.equal(await value("#test-draft"), "Next draft");
    assert.ok(await evaluate("document.querySelector('#test-composer [role=alert]').textContent.trim().length > 0"));
    await click(sendButton);
    await until(async () => (await submitted()).length === 4, "retry starts");
    await evaluate("window.__chatTest.resolve()");
    await until(async () => await value("#test-draft") === "", "retry clears draft");
    assert.equal(await evaluate("!!document.querySelector('#test-composer [role=alert]')"), false);
  });

  await check("disabled and generating states block sending and expose Stop", async () => {
    await type("Unsent message");
    await evaluate("window.__chatTest.setDisabled(true)");
    await until(() => evaluate("document.getElementById('test-draft').disabled"), "composer is disabled");
    assert.equal(await evaluate(`${query(sendButton)}.disabled`), true);
    await evaluate("document.getElementById('test-composer').requestSubmit()");
    assert.equal((await submitted()).length, 4);
    await evaluate("window.__chatTest.setDisabled(false); window.__chatTest.setGenerating(true)");
    await until(() => evaluate("!!document.querySelector('#test-composer button[aria-label=\"Stop response\"]')"), "Stop control renders");
    await focus("#test-draft");
    await key("Enter");
    assert.equal((await submitted()).length, 4);
    await click('#test-composer button[aria-label="Stop response"]');
    await until(() => evaluate("window.__chatTest.stops === 1"), "stop callback runs");
    assert.equal(await value("#test-draft"), "Unsent message");
    await until(() => evaluate(`!!${query(sendButton)}`), "send control returns");
    assert.equal((await submitted()).length, 4, "stopping does not submit the draft");
    assert.equal(await evaluate(`${query(sendButton)}.disabled`), false, "draft can be sent after stopping");
  });

  await check("sendOnEnter false keeps Enter available for multiline editing", async () => {
    await evaluate("window.__chatTest.setSendOnEnter(false)");
    await type("Button only");
    await key("Enter");
    assert.equal(await value("#test-draft"), "Button only\n");
    assert.equal((await submitted()).length, 4);
    await click(sendButton);
    await until(async () => (await submitted()).length === 5, "button-only composer sends");
    assert.equal(await evaluate("document.activeElement?.id"), "test-draft", "button send preserves editing focus");
    assert.equal((await submitted())[4], "Button only");
    await evaluate("window.__chatTest.resolve()");
    await until(async () => await value("#test-draft") === "", "button-only send succeeds");
    await evaluate("window.__chatTest.setSendOnEnter(true)");
  });

  await check("thread is an accessible log and automatically follows appended messages", async () => {
    assert.equal(await attr('#test-thread [data-slot="chat-thread-viewport"]', "role"), "log");
    assert.equal(await attr('#test-thread [data-slot="chat-thread-viewport"]', "aria-label"), "Test conversation");
    assert.equal(await attr("#test-thread", "data-direction"), "top-to-bottom", "omitted direction defaults to top-to-bottom");
    await assertThreadOrder("test-thread", false);
    assert.equal((await threadPosition("test-manual-reverse-thread")).top, 0, "reverse manual thread does not reposition on mount");
    await until(async () => (await threadPosition("test-thread")).bottom <= 2, "initial thread follows newest message");
    const beforeAppend = await threadPosition("test-thread");
    await evaluate("window.__chatTest.append()");
    await until(async () => { const position = await threadPosition("test-thread"); return position.height > beforeAppend.height && position.bottom <= 2; }, "append follows newest message");
    const before = await threadPosition("test-thread");
    await evaluate("document.getElementById('test-growing-content').style.height='120px'");
    await until(async () => (await threadPosition("test-thread")).height > before.height && (await threadPosition("test-thread")).bottom <= 2, "content resize follows newest message");
    assert.equal(await attr("#test-typing", "role"), "status");
    assert.ok(await evaluate("document.getElementById('test-typing').textContent.includes('Assistant is typing') || document.getElementById('test-typing').getAttribute('aria-label') === 'Assistant is typing'"));
  });

  await check("reading older messages holds the position and offers a jump to latest", async () => {
    await evaluate(`(() => { const el=${threadScroll("test-thread")}; el.scrollTop=0; el.dispatchEvent(new Event('scroll')); })()`);
    await until(() => evaluate("!!document.querySelector('#test-thread [data-slot=chat-thread-jump]')"), "jump control appears");
    assert.equal(await evaluate("document.querySelector('#test-thread [data-slot=chat-thread-jump]').textContent.trim()"), "Jump to latest");
    assert.equal(await evaluate("!!document.querySelector('#test-thread [data-slot=chat-thread-jump] .lucide-arrow-down')"), true, "default jump points down toward latest");
    const before = await threadPosition("test-thread");
    await evaluate("window.__chatTest.append()");
    await until(async () => (await threadPosition("test-thread")).height > before.height, "new message renders while reading history");
    assert.equal((await threadPosition("test-thread")).top, before.top);
    await evaluate("document.getElementById('test-growing-content').style.height='180px'");
    await pause(60);
    assert.equal((await threadPosition("test-thread")).top, before.top, "resizing content preserves history position");
    await click('#test-thread [data-slot="chat-thread-jump"]');
    await until(async () => (await threadPosition("test-thread")).bottom <= 2, "jump scrolls to latest");
    assert.equal(await evaluate("document.activeElement === document.querySelector('#test-thread [data-slot=chat-thread-viewport]')"), true, "jump transfers focus to the conversation");
  });

  await check("autoScroll false preserves an independently managed scroll position", async () => {
    const before = await threadPosition("test-manual-thread");
    assert.equal(before.top, 0, "manual thread does not jump on mount");
    await evaluate("window.__chatTest.append()");
    await until(async () => (await threadPosition("test-manual-thread")).height > before.height, "manual thread appends");
    assert.equal((await threadPosition("test-manual-thread")).top, before.top);
  });

  await check("direction changes preserve message state, DOM identity and keyboard focus", async () => {
    await click("#test-message-state");
    await until(() => evaluate("document.getElementById('test-message-state').textContent === 'Saved reactions: 1'"), "message action stores local state");
    await evaluate("void(window.__chatStateNode=document.getElementById('test-message-state'))");
    for (const direction of ["bottom-to-top", "top-to-bottom"]) {
      await evaluate(`window.__chatTest.setDirection(${JSON.stringify(direction)})`);
      await until(async () => await attr("#test-thread", "data-direction") === direction, "stateful message changes direction");
      assert.equal(await evaluate("document.getElementById('test-message-state').textContent"), "Saved reactions: 1", "message state survives reordering");
      assert.equal(await evaluate("document.getElementById('test-message-state') === window.__chatStateNode"), true, "message action is moved without remounting");
      assert.equal(await evaluate("document.activeElement === window.__chatStateNode"), true, "keyboard focus survives reordering");
    }
  });

  await check("bottom-to-top renders newest first and follows prepended messages and rapid growth", async () => {
    await evaluate("window.__chatTest.setDirection('bottom-to-top')");
    await until(async () => await attr("#test-thread", "data-direction") === "bottom-to-top" && (await threadPosition("test-thread")).top <= 2, "reverse layout positions latest at top");
    await assertThreadOrder("test-thread", true);
    const before = await threadPosition("test-thread");
    await evaluate("window.__chatTest.append()");
    await until(async () => { const current=await threadPosition("test-thread"); return current.height > before.height && current.top <= 2; }, "reverse append keeps latest visible");
    await evaluate(`(async () => {
      for(let index=0;index<8;index++) {
        document.getElementById('test-growing-content').style.height=(200+index*19)+'px';
        if(index%2===0) window.__chatTest.append();
        await new Promise(resolve=>requestAnimationFrame(resolve));
      }
    })()`);
    await until(async () => (await threadPosition("test-thread")).top <= 2, "rapid reverse resizes keep following latest");
    await assertThreadOrder("test-thread", true);
    assert.equal(await evaluate("!!document.querySelector('#test-thread [data-slot=chat-thread-jump]')"), false, "following reverse thread hides jump control");
  });

  await check("reverse history preserves the visible message while newer content is prepended", async () => {
    await readHistory("test-thread");
    const anchor = await visibleAnchor("test-thread");
    const before = await threadPosition("test-thread");
    await evaluate("window.__chatTest.append()");
    await until(async () => (await threadPosition("test-thread")).height > before.height, "reverse history receives a newer message");
    await until(async () => Math.abs(await anchorOffset("test-thread", anchor.number) - anchor.offset) <= 2, "prepending restores the reading anchor");
    await assertAnchorPreserved("test-thread", anchor, "prepending keeps the same message at the same visual offset");
    await evaluate("document.getElementById('test-growing-content').style.height='440px'");
    // ResizeObserver can run later than a fixed delay in headless Chrome.
    await until(async () => Math.abs(await anchorOffset("test-thread", anchor.number) - anchor.offset) <= 2, "streaming restores the reading anchor");
    await assertAnchorPreserved("test-thread", anchor, "streaming above the viewport preserves the reader's anchor");
    const jump = '#test-thread [data-slot="chat-thread-jump"]';
    assert.equal(await evaluate(`!!${query(jump)}.querySelector('.lucide-arrow-up')`), true, "reverse jump points toward latest at the top");
    const placement = await evaluate(`(() => { const button=${query(jump)}.getBoundingClientRect(), thread=document.getElementById('test-thread').getBoundingClientRect(); return button.top-thread.top; })()`);
    assert.ok(placement >= 0 && placement <= 24, "reverse jump is placed at the top");
    await click(jump);
    await until(async () => (await threadPosition("test-thread")).top <= 2, "reverse jump reaches latest at top");
    assert.equal(await evaluate("document.activeElement === document.querySelector('#test-thread [data-slot=chat-thread-viewport]')"), true, "reverse jump restores conversation focus");
  });

  await check("switching either direction returns history readers to the new latest edge", async () => {
    for (const direction of ["top-to-bottom", "bottom-to-top"]) {
      await readHistory("test-thread");
      await evaluate(`window.__chatTest.setDirection(${JSON.stringify(direction)})`);
      await until(async () => {
        const current=await threadPosition("test-thread");
        return await attr("#test-thread", "data-direction") === direction && (direction === "top-to-bottom" ? current.bottom : current.top) <= 2;
      }, "direction switch returns to latest");
      await assertThreadOrder("test-thread", direction === "bottom-to-top");
      assert.equal(await evaluate("!!document.querySelector('#test-thread [data-slot=chat-thread-jump]')"), false);
      const before = await threadPosition("test-thread");
      await evaluate("window.__chatTest.append()");
      await until(async () => {
        const current=await threadPosition("test-thread");
        return current.height > before.height && (direction === "top-to-bottom" ? current.bottom : current.top) <= 2;
      }, "direction switch resumes automatic following");
    }
  });

  await check("reverse autoScroll false preserves history and only repositions on a direction switch", async () => {
    const id = "test-manual-reverse-thread";
    await assertThreadOrder(id, true);
    await readHistory(id);
    const anchor = await visibleAnchor(id);
    const before = await threadPosition(id);
    await evaluate("window.__chatTest.append()");
    await until(async () => (await threadPosition(id)).height > before.height, "manual reverse thread receives new content");
    await pause(80);
    assert.ok(Math.abs(await anchorOffset(id, anchor.number) - anchor.offset) <= 2, "manual reverse content does not pull history readers to latest");
    for (const direction of ["top-to-bottom", "bottom-to-top"]) {
      await evaluate(`window.__chatTest.setManualDirection(${JSON.stringify(direction)})`);
      await until(async () => {
        const current=await threadPosition(id);
        return await attr(`#${id}`, "data-direction") === direction && (direction === "top-to-bottom" ? current.bottom : current.top) <= 2;
      }, "manual direction switch intentionally repositions to latest");
      const latest = await visibleAnchor(id);
      const beforeAppend = await threadPosition(id);
      await evaluate("window.__chatTest.append()");
      await until(async () => (await threadPosition(id)).height > beforeAppend.height, "manual thread receives content after switch");
      await pause(80);
      assert.ok(Math.abs(await anchorOffset(id, latest.number) - latest.offset) <= 2, "content after a direction switch remains manual");
      const current = await threadPosition(id);
      assert.ok((direction === "top-to-bottom" ? current.bottom : current.top) > 64, "manual updates do not follow even when previously at latest");
    }
  });

  await check("chat content remains visible in both themes and fits a narrow viewport", async () => {
    for (const mode of ["light", "dark", "mobile"]) {
      await evaluate(`document.documentElement.classList.toggle('dark', ${mode === "dark"})`);
      await send("Emulation.setDeviceMetricsOverride", { width: mode === "mobile" ? 390 : 1280, height: 1000, deviceScaleFactor: 1, mobile: mode === "mobile" });
      await until(() => evaluate("/^oklch\\(/.test(getComputedStyle(document.getElementById('test-composer')).backgroundColor)"), "fixture theme color settles");
      assert.equal(await evaluate("document.documentElement.scrollWidth <= window.innerWidth"), true, `${mode} has no page overflow`);
      const colors = await evaluate("(() => { const style=getComputedStyle(document.getElementById('test-composer')); return {background:style.backgroundColor,text:style.color}; })()");
      assert.notEqual(colors.background, colors.text, `${mode} text contrasts with background`);
      const toolColors = await evaluate("(() => { const summary=getComputedStyle(document.querySelector('#test-tool-disclosure [data-slot=chat-tool-call-trigger]')), output=getComputedStyle(document.getElementById('test-tool-output')); return {summaryText:summary.color,outputBackground:output.backgroundColor,outputText:output.color}; })()");
      assert.match(toolColors.summaryText, /^oklch\(/, `${mode} tool summary uses a theme foreground`);
      assert.match(toolColors.outputBackground, /^(oklch|oklab)\(/, `${mode} tool output uses a theme background`);
      assert.notEqual(toolColors.outputBackground, toolColors.outputText, `${mode} tool output text contrasts with its background`);
      for (const selector of ["#test-tool-output", '#test-tool-diff [data-slot="chat-tool-diff-viewport"]']) {
        const overflow = await evaluate(`(() => { const el=${query(selector)}, bounds=el.getBoundingClientRect(); return {scrollable:el.scrollWidth>el.clientWidth,overflow:getComputedStyle(el).overflowX,left:bounds.left,right:bounds.right}; })()`);
        assert.equal(overflow.scrollable, true, `${mode} long tool lines remain inside a scroll area`);
        assert.equal(overflow.overflow, "auto", `${mode} long tool lines can be scrolled`);
        assert.ok(overflow.left >= 0 && overflow.right <= (mode === "mobile" ? 390 : 1280), `${mode} tool content fits the viewport`);
      }
      if (process.env.BROWSER_SCREENSHOTS_DIR) {
        await evaluate("window.scrollTo({top:0,behavior:'instant'})");
        await pause(250);
        const path = resolve(process.env.BROWSER_SCREENSHOTS_DIR, `design-system-chat-${mode}.png`);
        const screenshot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
        await writeFile(path, Buffer.from(screenshot.data, "base64"));
        console.log(`Screenshot: ${path}`);
      }
    }
  });

  assert.deepEqual(errors, [], "browser exceptions and console errors");
  console.log(`\n${checks.length} chat browser scenarios passed; no exceptions or console errors.`);
} catch (error) {
  console.error(`Browser check failed after ${checks.length} completed scenarios.`);
  console.error(errors.length ? `Collected browser errors:\n${errors.join("\n\n")}` : "No browser errors were collected.");
  if (collectFailureDetails && socket?.readyState === WebSocket.OPEN) {
    try { await collectFailureDetails(); }
    catch (diagnosticError) { console.error(`Could not collect browser diagnostics: ${diagnosticError.message}`); }
  }
  throw error;
} finally {
  socket?.close();
  chrome.kill();
  await chrome.exited;
  await rm(profile, { recursive: true, force: true });
}
