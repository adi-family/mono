import assert from "node:assert/strict";
import { test } from "./helpers.mjs";

test("notice dismissal hides the notice and emits one event outside its containing shadow root", async ({ page, mount }) => {
  await mount('<div id="container"></div>');
  await page.evaluate(() => {
    const root = document.querySelector("#container").attachShadow({ mode: "open" });
    root.innerHTML = '<adi-notice dismissible lead="Update">A new release is available</adi-notice>';
    window.dismissals = [];
    document.body.addEventListener("dismiss", event => window.dismissals.push({
      bubbles: event.bubbles, composed: event.composed, target: event.target.id,
    }));
  });
  await page.getByRole("button", { name: "Dismiss", exact: true }).click();
  assert.equal(await page.locator("adi-notice").isVisible(), false);
  assert.deepEqual(await page.evaluate(() => window.dismissals), [{ bubbles: true, composed: true, target: "container" }]);
});

test("grant removal reports the value and lets the caller decide when to remove it", async ({ page, mount }) => {
  await mount('<adi-grant value="read:files">Read files</adi-grant>');
  await page.evaluate(() => {
    window.removals = [];
    document.body.addEventListener("remove", event => window.removals.push(event.detail));
  });
  await page.getByRole("button", { name: "Remove read:files", exact: true }).press("Enter");
  assert.deepEqual(await page.evaluate(() => window.removals), [{ value: "read:files" }]);
  assert.equal(await page.locator("adi-grant").isVisible(), true);
});

test("tool call toggles via keyboard and reflects external open changes without duplicate events", async ({ page, mount }) => {
  await mount('<adi-tool-call tool="Read" command="notes.txt"><p>File contents</p></adi-tool-call>');
  await page.evaluate(() => {
    window.toggles = [];
    document.body.addEventListener("toggle", event => {
      if (event instanceof CustomEvent) window.toggles.push(event.detail);
    });
  });
  const header = page.locator("adi-tool-call button");
  assert.equal(await header.getAttribute("aria-expanded"), "false");
  assert.equal(await page.getByText("File contents").isVisible(), false);
  await header.press("Enter");
  assert.equal(await header.getAttribute("aria-expanded"), "true");
  assert.equal(await page.getByText("File contents").isVisible(), true);
  await page.locator("adi-tool-call").evaluate(node => node.setAttribute("command", "updated.txt"));
  assert.equal(await header.getAttribute("aria-expanded"), "true");
  await header.press("Space");
  assert.equal(await header.getAttribute("aria-expanded"), "false");
  await page.locator("adi-tool-call").evaluate(node => node.setAttribute("open", ""));
  assert.equal(await page.getByText("File contents").isVisible(), true);
  assert.deepEqual(await page.evaluate(() => window.toggles), [{ open: true }, { open: false }]);
});

test("code copy writes the current code and reports success after the clipboard resolves", async ({ page, mount }) => {
  await mount('<adi-code copy>\n  const value = 1;\n</adi-code>');
  await page.evaluate(() => {
    window.writes = [];
    window.copies = [];
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: {
      writeText: text => {
        window.writes.push(text);
        return new Promise(resolve => { window.finishCopy = resolve; });
      },
    } });
    document.body.addEventListener("copy", event => {
      if (event instanceof CustomEvent) window.copies.push(event.detail);
    });
  });
  await page.getByRole("button", { name: "Copy", exact: true }).click();
  assert.deepEqual(await page.evaluate(() => window.writes), ["  const value = 1;"]);
  assert.deepEqual(await page.evaluate(() => window.copies), []);
  await page.evaluate(() => window.finishCopy());
  assert.deepEqual(await page.evaluate(() => window.copies), [{ code: "  const value = 1;" }]);
  assert.equal(await page.locator("adi-code .copy adi-icon").getAttribute("name"), "check");
});

test("code copy does not emit success when the clipboard rejects", async ({ page, mount }) => {
  await mount('<adi-code copy>copy me</adi-code>');
  await page.evaluate(() => {
    window.copies = [];
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: {
      writeText: async () => { throw new Error("Clipboard unavailable"); },
    } });
    document.body.addEventListener("copy", event => {
      if (event instanceof CustomEvent) window.copies.push(event.detail);
    });
  });
  await page.getByRole("button", { name: "Copy", exact: true }).click();
  assert.deepEqual(await page.evaluate(() => window.copies), []);
  assert.equal(await page.locator("adi-code .copy adi-icon").getAttribute("name"), "copy");
});

test("reconnecting an element preserves its state without multiplying event listeners", async ({ page, mount }) => {
  await mount('<adi-tool-call tool="Read">Result</adi-tool-call>');
  await page.evaluate(() => {
    window.toggles = [];
    document.body.addEventListener("toggle", event => {
      if (event instanceof CustomEvent) window.toggles.push(event.detail);
    });
  });
  await page.locator("adi-tool-call button").click();
  await page.evaluate(() => {
    const element = document.querySelector("adi-tool-call");
    element.remove();
    document.body.append(element);
  });
  assert.equal(await page.locator("adi-tool-call button").getAttribute("aria-expanded"), "true");
  await page.locator("adi-tool-call button").click();
  assert.deepEqual(await page.evaluate(() => window.toggles), [{ open: true }, { open: false }]);
});
