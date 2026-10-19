import assert from "node:assert/strict";
import { test } from "./helpers.mjs";

const RUNS = [
  { run_id: "r1", started_at: 1_000, last_activity: 2_000, message: "First message\nsecond line", title: null, running: false, hidden: false, starred: false },
  { run_id: "r2", started_at: 3_000, last_activity: 4_000, message: "Second run opener", title: "Named run", running: true, hidden: false, starred: true },
];

// window.fetch is overridden directly (not Playwright network routing, which the harness aborts
// for every http(s) request) so the stub runs regardless of that catch-all.
async function stubApi(page, runs = RUNS) {
  await page.evaluate((runs) => {
    window.__requests = [];
    window.fetch = async (url) => {
      const path = new URL(url, location.href).pathname;
      window.__requests.push(path);
      const json = (body, status = 200) => new Response(JSON.stringify(body), { status });
      if (path === "/api/agents") return json({ agents: [{ name: "assistant", starred: true, running: false }] });
      if (path === "/api/fleet/nodes") return json({ nodes: [] });
      if (path === "/api/agents/runs") return json({ runs });
      if (path === "/api/agents/goals") return json({ goals: [] });
      if (path === "/api/agents/run/peek") {
        return json({ running: false, output: "", answerable: true, caps: { images: false }, turns: [] });
      }
      return json({}, 404);
    };
  }, runs);
}

const HOST = '<div id="host" style="width: 760px; height: 480px"><adi-chat agent="assistant" sidebar></adi-chat></div>';

test("sidebar lists the agent's conversations from the stubbed API, newest first", async ({ page, mount }) => {
  await stubApi(page);
  await mount(HOST);
  const rows = page.locator("adi-chat .rows .row");
  assert.equal(await rows.count(), 2);
  assert.equal(await rows.nth(0).locator(".name").textContent(), "Named run");
  assert.equal(await rows.nth(0).locator(".meta").textContent(), "working");
  assert.equal(await rows.nth(0).locator(".live").count(), 1);
  assert.equal(await rows.nth(0).locator(".star").count(), 1);
  assert.equal(await rows.nth(1).locator(".name").textContent(), "First message");
  assert.equal(await rows.nth(1).locator(".star").count(), 0);
  assert.equal(await page.locator("adi-chat .fresh").isVisible(), true);
  // The agent's latest run opens automatically; its row is highlighted.
  assert.equal(await rows.nth(0).getAttribute("class"), "row active");
});

test("selecting a row in the rail opens it and fires place", async ({ page, mount }) => {
  await stubApi(page);
  await mount(HOST);
  await page.evaluate(() => {
    window.places = [];
    document.body.addEventListener("place", (event) => window.places.push(event.detail));
  });
  const rows = page.locator("adi-chat .rows .row");
  await rows.nth(1).click();
  assert.deepEqual(await page.evaluate(() => window.places.at(-1)), {
    node: null, agent: "assistant", run: "r1", title: "First message",
  });
  assert.equal(await rows.nth(1).getAttribute("class"), "row active");
  assert.equal(await rows.nth(0).getAttribute("class"), "row");
});

test("New conversation opens a fresh run, highlighting no row", async ({ page, mount }) => {
  await stubApi(page);
  await mount(HOST);
  await page.evaluate(() => {
    window.places = [];
    document.body.addEventListener("place", (event) => window.places.push(event.detail));
  });
  await page.locator("adi-chat .fresh").click();
  assert.deepEqual(await page.evaluate(() => window.places.at(-1)), {
    node: null, agent: "assistant", run: null, title: "",
  });
  assert.equal(await page.locator("adi-chat .rows .row.active").count(), 0);
  assert.equal(await page.locator("adi-chat .empty").textContent(), "A new conversation with assistant starts with what you write above.");
});

test("the rail collapses under 640px of the element's own width, and the bar toggle overrides it", async ({ page, mount }) => {
  await stubApi(page);
  await mount(HOST);
  const list = page.locator("adi-chat .list");
  assert.equal(await list.isHidden(), false);

  await page.locator("adi-chat .railToggle").click();
  assert.equal(await list.isHidden(), true);
  await page.locator("adi-chat .railToggle").click();
  assert.equal(await list.isHidden(), false);

  await page.evaluate(() => { document.querySelector("#host").style.width = "500px"; });
  await page.waitForFunction(() => document.querySelector("adi-chat").shadowRoot.querySelector(".list").hidden === true);
  assert.equal(await list.isHidden(), true);

  await page.locator("adi-chat .railToggle").click();
  assert.equal(await list.isHidden(), false);

  await page.evaluate(() => { document.querySelector("#host").style.width = "760px"; });
  await page.waitForFunction(() => document.querySelector("adi-chat").shadowRoot.querySelector(".list").hidden === false);
});
