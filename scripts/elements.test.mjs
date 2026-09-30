#!/usr/bin/env node
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const { values } = parseArgs({ options: {
  elements: { type: "string", default: "target/elements" },
  capture: { type: "string" },
  compare: { type: "string" },
  shared: { type: "boolean", default: false },
  screenshot: { type: "string" },
} });
let chromium;
try {
  ({ chromium } = await import("playwright"));
} catch {
  ({ chromium } = await import("../apps/docs/node_modules/playwright/index.mjs"));
}
const chrome = process.env.ELEMENTS_CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const browser = await chromium.launch({
  ...(existsSync(chrome) ? { executablePath: chrome } : {}),
  headless: true,
  args: ["--allow-file-access-from-files"],
});
const scratch = await mkdtemp(join(tmpdir(), "adi-elements-test-"));
try {
  const fixture = join(scratch, "index.html");
  await writeFile(fixture, `<!doctype html><html><head><meta charset="utf-8">
    <link rel="stylesheet" href="${pathToFileURL(join(root, "design/tokens.css"))}">
    <style>body { margin: 24px; background: var(--bg); color: var(--ink); }
      #cases { display: grid; gap: 16px; } #chat { height: 520px; }</style>
    <script>window.fetch = async () => new Response('{}', { status: 503 });</script>
    <script type="module" src="${pathToFileURL(join(resolve(root, values.elements), "adi-elements.js"))}"></script>
    </head><body><div id="cases">
    <form id="form"><adi-field label="Name"><adi-input id="input" name="name" value="initial"></adi-input></adi-field>
      <adi-select id="select" name="mode" value="b"><option value="a">First</option><option value="b">Second</option></adi-select>
      <adi-textarea id="textarea" name="text" value="default"></adi-textarea></form>
    <adi-segmented id="segmented" value="a"><button value="a">First</button><button value="b">Second</button></adi-segmented>
    <adi-modal id="modal" label="Example"><p>Dialog content</p></adi-modal>
    <adi-composer id="composer" attach><adi-mic slot="tools"></adi-mic></adi-composer>
    <adi-markdown id="markdown"></adi-markdown>
    <adi-message id="message" role="user" queued></adi-message>
    <adi-note id="note"></adi-note><adi-ask id="ask"></adi-ask>
    <adi-tool-run id="tool"></adi-tool-run>
    <adi-transcript id="transcript"><div slot="lead">Lead</div><div slot="foot">Foot</div></adi-transcript>
    <adi-chat id="chat"></adi-chat>
    <adi-button id="hidden" hidden>Hidden</adi-button>
    </div><adi-gallery></adi-gallery></body></html>`);
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.route(/^https?:/, route => route.abort());
  await page.goto(pathToFileURL(fixture).href);
  await page.waitForFunction(() => document.querySelector("adi-gallery")?.shadowRoot?.querySelector(".icons adi-icon"));
  await page.evaluate(() => {
    const byId = id => document.getElementById(id);
    const source = "# Heading\n\nA **bold** word and `code`.\n\n| Key | Value |\n| --- | ---: |\n| alpha | 1 |\n\n```js\nconst x = 1;\n```\n\n[safe](https://example.com) [unsafe](javascript:alert)\n\n<img src=x onerror=alert(1)>";
    byId("markdown").source = source;
    byId("message").body = source;
    byId("note").note = { head: [{ text: "Note" }, { code: "abc" }], body: source };
    byId("ask").ask = { id: "question", note: "**Choose** an option.", questions: [{ question: "Which option?", options: [{ label: "First" }, { label: "Second" }] }] };
    byId("tool").run = { id: "tool", count: 1, tools: ["Read"], preview: "file.txt", state: "ok", open: true, calls: [{ name: "Read", params: [["path", "file.txt"]], state: "ok", result: "Contents" }] };
    byId("transcript").entries = [{ key: "message", kind: "said", role: "agent", body: "**Transcript** message" }];
  });
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));

  const snapshots = {};
  for (const [name, width, theme] of [["desktop", 1280, "dark"], ["mobile", 390, "dark"], ["light", 1280, "light"]]) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme);
    snapshots[name] = await page.evaluate(() => {
      const properties = ["display", "position", "box-sizing", "color", "background-color", "font-family", "font-size", "font-weight", "line-height", "white-space", "text-align", "text-decoration-line", "border-top-width", "border-top-color", "border-top-style", "border-radius", "padding-top", "padding-right", "padding-bottom", "padding-left", "margin-top", "margin-right", "margin-bottom", "margin-left", "gap", "flex-direction", "align-items", "justify-content", "flex-grow", "flex-shrink", "flex-basis", "grid-template-columns", "overflow-x", "overflow-y", "opacity", "cursor", "animation-name", "max-width", "min-width", "max-height", "min-height"];
      const snapshot = {};
      function visit(container, path) {
        [...container.children].forEach((node, index) => {
          const key = `${path}/${node.localName}[${index}]`;
          if (node.localName !== "path") {
            const style = getComputedStyle(node);
            snapshot[key] = Object.fromEntries(properties.map(property => [property, style.getPropertyValue(property)]));
          }
          if (node.shadowRoot) visit(node.shadowRoot, `${key}::shadow`);
          visit(node, key);
        });
      }
      visit(document.body, "body");
      return snapshot;
    });
  }
  if (values.capture) await writeFile(resolve(values.capture), JSON.stringify(snapshots));
  if (values.compare) {
    const previous = JSON.parse(await readFile(resolve(values.compare), "utf8"));
    const changes = [];
    for (const [mode, nodes] of Object.entries(previous)) {
      for (const [path, styles] of Object.entries(nodes)) {
        for (const [property, value] of Object.entries(styles)) {
          const actual = snapshots[mode]?.[path]?.[property];
          if (actual !== value) changes.push(`${mode} ${path} ${property}: ${value} -> ${actual}`);
        }
      }
      assert.equal(Object.keys(snapshots[mode]).length, Object.keys(nodes).length, `${mode}: DOM node count changed`);
    }
    assert.equal(changes.length, 0, `Computed styles changed:\n${changes.slice(0, 30).join("\n")}\n(${changes.length} differences)`);
  }

  const checks = await page.evaluate(async shared => {
    const checked = [];
    const check = (condition, name) => { if (!condition) throw new Error(name); checked.push(name); };
    const byId = id => document.getElementById(id);
    const input = byId("input");
    const control = input.shadowRoot.querySelector("input");
    const events = [];
    input.addEventListener("input", event => events.push([event.type, event.target === input]));
    input.addEventListener("change", event => events.push([event.type, event.target === input]));
    control.focus();
    control.value = "edited";
    control.setSelectionRange(1, 3);
    control.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
    control.dispatchEvent(new Event("change", { bubbles: true }));
    input.setAttribute("placeholder", "Changed placeholder");
    check(input.shadowRoot.activeElement === control && control.selectionStart === 1 && control.selectionEnd === 3 && input.value === "edited", "input focus and value survive updates");
    check(JSON.stringify(events) === JSON.stringify([["input", true], ["change", true]]), "form events fire once and retarget to the custom element");
    check(JSON.stringify([...new FormData(byId("form"))]) === JSON.stringify([["name", "edited"], ["mode", "b"], ["text", "default"]]), "form controls submit exactly once");
    byId("form").reset();
    check(input.value === "initial" && byId("select").value === "b" && byId("textarea").value === "default", "form reset restores defaults");
    const segmented = byId("segmented");
    segmented.shadowRoot.querySelector('[value="b"]').click();
    check(segmented.value === "b" && segmented.shadowRoot.querySelector('[value="b"]').getAttribute("aria-pressed") === "true", "segmented choices update");
    const modal = byId("modal");
    check(modal.shadowRoot.querySelector(".foot").hidden, "empty modal footer stays hidden");
    const footer = document.createElement("adi-button");
    footer.slot = "foot";
    footer.textContent = "Done";
    modal.append(footer);
    await new Promise(resolve => requestAnimationFrame(resolve));
    check(!modal.shadowRoot.querySelector(".foot").hidden, "named slots update after insertion");
    modal.show();
    check(modal.shadowRoot.querySelector("dialog").open, "modal opens native dialog");
    modal.close();
    check(!modal.shadowRoot.querySelector("dialog").open, "modal closes native dialog");
    const markdown = byId("markdown").shadowRoot;
    check(markdown.querySelector("strong")?.textContent === "bold" && markdown.querySelectorAll("table td").length === 2 && !!markdown.querySelector("pre code"), "Markdown renders inline, table and code content");
    check(!markdown.querySelector("img, script, a[href^='javascript:']"), "Markdown preserves safe rendering");
    check(getComputedStyle(byId("hidden")).display === "none", "hidden custom elements remain hidden");
    const composer = byId("composer");
    const tools = composer.shadowRoot.querySelector('slot[name="tools"]');
    check(tools.assignedElements()[0]?.localName === "adi-mic", "composer projects tool slot");
    let sent;
    composer.addEventListener("send", event => sent = event.detail.text);
    composer.value = "Hello";
    composer.shadowRoot.querySelector(".send").click();
    check(sent === "Hello", "composer sends typed message");
    if (shared) {
      const sheets = new Set();
      let roots = 0;
      function visit(container) {
        for (const node of container.children) {
          if (node.shadowRoot) {
            roots++;
            check(node.shadowRoot.adoptedStyleSheets.length === 1, `${node.localName} adopts one sheet`);
            sheets.add(node.shadowRoot.adoptedStyleSheets[0]);
            visit(node.shadowRoot);
          }
          visit(node);
        }
      }
      visit(document.body);
      check(roots > 100 && sheets.size === 1, "all instances reuse the same stylesheet object");
    }
    return checked.length;
  }, values.shared);
  await page.setViewportSize({ width: 390, height: 900 });
  assert.equal(await page.locator("#composer textarea").evaluate(node => getComputedStyle(node).fontSize), "16px", "mobile composer uses readable input text");
  assert.deepEqual(errors, [], "no browser errors");
  if (values.screenshot) {
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.evaluate(() => document.documentElement.dataset.theme = "dark");
    await page.locator("adi-gallery").screenshot({ path: resolve(values.screenshot) });
  }
  console.log(`elements: ${checks + 2} browser checks passed; ${Object.keys(snapshots.desktop).length} nodes checked at desktop/mobile/light${values.compare ? "; computed styles match baseline" : ""}`);
} finally {
  await browser.close();
  await rm(scratch, { recursive: true, force: true });
}
