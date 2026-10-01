import assert from "node:assert/strict";
import { mkdtemp, access, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test as nodeTest } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { launchElementsBrowser } from "../elements-browser.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
let browser;
let scratch;
let fixture;

before(async () => {
  const entry = join(root, "target/elements/adi-elements.js");
  await access(entry).catch(() => {
    throw new Error("Build elements first with: scripts/elements.sh build");
  });
  scratch = await mkdtemp(join(tmpdir(), "adi-elements-behavior-"));
  fixture = pathToFileURL(join(scratch, "index.html")).href;
  await writeFile(new URL(fixture), `<!doctype html><html><head><meta charset="utf-8">
    <link rel="stylesheet" href="${pathToFileURL(join(root, "design/tokens.css"))}">
    <script type="module" src="${pathToFileURL(entry)}"></script>
    </head><body></body></html>`);
  browser = await launchElementsBrowser();
});

after(async () => {
  try {
    await browser?.close();
  } finally {
    if (scratch) await rm(scratch, { recursive: true, force: true });
  }
});

// Fresh context, DOM, storage, and event listeners for every named test. No HTTP server.
export function test(name, body) {
  return nodeTest(name, { timeout: 20_000 }, async (t) => {
    const context = await browser.newContext({ viewport: { width: 1000, height: 800 } });
    t.after(() => context.close());
    const page = await context.newPage();
    page.setDefaultTimeout(3000);
    const errors = [];
    page.on("pageerror", error => errors.push(error.message));
    await context.route(/^https?:/, route => route.abort());
    await page.goto(fixture);
    await page.waitForFunction(() => customElements.get("adi-gallery"));
    const mount = async (html) => {
      await page.evaluate(async (markup) => {
        document.body.innerHTML = markup;
        // Child/slot observers settle before interacting with a newly mounted element.
        await new Promise(resolve => requestAnimationFrame(resolve));
      }, html);
    };
    await body({ page, mount });
    assert.deepEqual(errors, [], "no uncaught browser errors");
  });
}
