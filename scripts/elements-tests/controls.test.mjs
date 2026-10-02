import assert from "node:assert/strict";
import { test } from "./helpers.mjs";

test("controls emit one input and change event through the shadow boundary", async ({ page, mount }) => {
  await mount(`<form id="form">
    <adi-input id="name" name="name" value="initial"></adi-input>
    <adi-textarea id="note" name="note" value="default"></adi-textarea>
    <adi-select id="mode" name="mode" value="a"><option value="a">First</option><option value="b">Second</option></adi-select>
  </form>`);
  await page.evaluate(() => {
    window.events = [];
    for (const type of ["input", "change"]) {
      document.querySelector("form").addEventListener(type, event => {
        window.events.push({ type: event.type, target: event.target.id, value: event.target.value, composed: event.composed });
      });
    }
  });

  await page.locator("#name input").fill("Ada");
  await page.locator("#note textarea").fill("A new note");
  await page.locator("#mode select").focus();
  await page.locator("#mode select").selectOption("b");

  assert.deepEqual(await page.evaluate(() => window.events), [
    { type: "input", target: "name", value: "Ada", composed: true },
    { type: "change", target: "name", value: "Ada", composed: true },
    { type: "input", target: "note", value: "A new note", composed: true },
    { type: "change", target: "note", value: "A new note", composed: true },
    { type: "input", target: "mode", value: "b", composed: true },
    { type: "change", target: "mode", value: "b", composed: true },
  ]);
});

test("native form submission includes each edited control once and reset restores value defaults", async ({ page, mount }) => {
  await mount(`<form id="form">
    <adi-input id="name" name="name" value="initial"></adi-input>
    <adi-textarea id="note" name="note" value="default"></adi-textarea>
    <adi-select id="mode" name="mode" value="b"><option value="a">First</option><option value="b">Second</option></adi-select>
    <button type="submit">Submit</button><button type="reset">Reset</button>
  </form>`);
  await page.evaluate(() => {
    window.submissions = [];
    document.querySelector("form").addEventListener("submit", event => {
      event.preventDefault();
      window.submissions.push([...new FormData(event.target)]);
    });
  });
  await page.locator("#name input").fill("edited");
  await page.locator("#note textarea").fill("new note");
  await page.locator("#mode select").selectOption("a");
  await page.getByRole("button", { name: "Submit", exact: true }).click();

  assert.deepEqual(await page.evaluate(() => window.submissions), [
    [["name", "edited"], ["note", "new note"], ["mode", "a"]],
  ]);
  await page.getByRole("button", { name: "Reset", exact: true }).click();
  assert.deepEqual(await page.evaluate(() => ({
    values: ["name", "note", "mode"].map(id => document.getElementById(id).value),
    submitted: [...new FormData(document.querySelector("form"))],
    associated: ["name", "note", "mode"].every(id => document.getElementById(id).form === document.querySelector("form")),
  })), {
    values: ["initial", "default", "b"],
    submitted: [["name", "initial"], ["note", "default"], ["mode", "b"]],
    associated: true,
  });
});

test("disabled controls are omitted from FormData and readonly input resists typing", async ({ page, mount }) => {
  await mount(`<form>
    <adi-input id="disabled-input" name="disabled-input" value="hidden" disabled></adi-input>
    <adi-textarea id="disabled-textarea" name="disabled-textarea" value="hidden" disabled></adi-textarea>
    <adi-select id="disabled-select" name="disabled-select" value="a" disabled><option value="a">First</option></adi-select>
    <adi-input id="readonly" name="readonly" value="retained" readonly></adi-input>
  </form>`);
  assert.equal(await page.locator("#disabled-input input").isDisabled(), true);
  assert.equal(await page.locator("#disabled-textarea textarea").isDisabled(), true);
  assert.equal(await page.locator("#disabled-select select").isDisabled(), true);

  await page.locator("#readonly input").click();
  await page.locator("#readonly input").press("End");
  await page.locator("#readonly input").press("x");
  assert.equal(await page.locator("#readonly input").inputValue(), "retained");
  assert.deepEqual(await page.evaluate(() => [...new FormData(document.querySelector("form"))]), [["readonly", "retained"]]);

  await page.locator("#disabled-input").evaluate(node => node.removeAttribute("disabled"));
  await page.locator("#disabled-input input").fill("enabled");
  assert.deepEqual(await page.evaluate(() => [...new FormData(document.querySelector("form"))]), [
    ["disabled-input", "enabled"], ["readonly", "retained"],
  ]);
});

test("field labels focus the slotted input, textarea, and select", async ({ page, mount }) => {
  await mount(`<adi-field id="input-field" label="Name"><adi-input></adi-input></adi-field>
    <adi-field id="textarea-field" label="Note"><adi-textarea></adi-textarea></adi-field>
    <adi-field id="select-field" label="Mode"><adi-select value="a"><option value="a">First</option></adi-select></adi-field>`);
  for (const [id, tag] of [["input-field", "input"], ["textarea-field", "textarea"], ["select-field", "select"]]) {
    await page.locator(`#${id} .label`).click();
    assert.equal(await page.locator(`#${id} ${tag}`).evaluate(node => node.getRootNode().activeElement === node), true, `${tag} receives focus`);
  }
});

test("input and textarea preserve focus, selection, and edits when attributes change", async ({ page, mount }) => {
  await mount(`<adi-input id="input" value="initial"></adi-input><adi-textarea id="textarea" value="initial"></adi-textarea>`);
  for (const [id, tag] of [["input", "input"], ["textarea", "textarea"]]) {
    const control = page.locator(`#${id} ${tag}`);
    await control.fill("edited text");
    await control.press("Home");
    await control.press("Shift+ArrowRight");
    await control.press("Shift+ArrowRight");
    const before = await control.evaluate(node => ({ start: node.selectionStart, end: node.selectionEnd }));
    assert.notEqual(before.start, before.end, "keyboard selected some text");
    await page.locator(`#${id}`).evaluate(host => {
      window.previousControl = host.control;
      host.setAttribute("placeholder", "Updated placeholder");
      host.setAttribute("label", "Updated label");
      host.setAttribute("required", "");
    });
    assert.deepEqual(await control.evaluate(node => ({
      start: node.selectionStart, end: node.selectionEnd,
      focused: node.getRootNode().activeElement === node,
      sameNode: node === window.previousControl,
      value: node.value,
    })), { ...before, focused: true, sameNode: true, value: "edited text" });
  }
});

test("programmatic control values update FormData without emitting user events", async ({ page, mount }) => {
  await mount(`<form><adi-input id="input" name="input"></adi-input><adi-textarea id="textarea" name="textarea"></adi-textarea>
    <adi-select id="select" name="select" value="a"><option value="a">First</option><option value="b">Second</option></adi-select></form>`);
  const result = await page.evaluate(() => {
    const events = [];
    const form = document.querySelector("form");
    form.addEventListener("input", event => events.push(event.type));
    form.addEventListener("change", event => events.push(event.type));
    document.getElementById("input").value = 42;
    document.getElementById("textarea").value = null;
    document.getElementById("select").value = "b";
    return { events, data: [...new FormData(form)] };
  });
  assert.deepEqual(result, { events: [], data: [["input", "42"], ["textarea", ""], ["select", "b"]] });
});

test("refreshing select options preserves the user's selection and submitted value", async ({ page, mount }) => {
  await mount(`<form><adi-select id="mode" name="mode" value="a"><option value="a">First</option><option value="b">Second</option></adi-select></form>`);
  await page.locator("#mode select").selectOption("b");
  await page.locator("#mode").evaluate(node => {
    node.options = [{ value: "b", label: "Updated second" }, { value: "c", label: "Third" }];
  });
  assert.equal(await page.locator("#mode select").inputValue(), "b");
  assert.deepEqual(await page.locator("#mode").evaluate(node => node.options), [
    { value: "b", label: "Updated second" }, { value: "c", label: "Third" },
  ]);
  assert.deepEqual(await page.evaluate(() => [...new FormData(document.querySelector("form"))]), [["mode", "b"]]);
});

test("segmented choices update pressed state and emit one change only when the value changes", async ({ page, mount }) => {
  await mount(`<adi-segmented id="mode" value="a"><button value="a">First</button><button value="b"><span>Second</span></button></adi-segmented>`);
  await page.evaluate(() => {
    window.changes = [];
    document.body.addEventListener("change", event => window.changes.push({ target: event.target.id, value: event.detail.value, composed: event.composed }));
  });
  await page.locator("#mode button[value=b] span").click();
  await page.locator("#mode button[value=b]").click();
  assert.deepEqual(await page.locator("#mode").evaluate(node => ({
    value: node.value,
    pressed: node.options.map(option => option.getAttribute("aria-pressed")),
  })), { value: "b", pressed: ["false", "true"] });
  assert.deepEqual(await page.evaluate(() => window.changes), [{ target: "mode", value: "b", composed: true }]);

  await page.locator("#mode").evaluate(node => { node.value = "a"; });
  assert.equal(await page.locator("#mode button[value=a]").getAttribute("aria-pressed"), "true");
  assert.equal(await page.evaluate(() => window.changes.length), 1, "programmatic selection does not emit a user change");
});

test("segmented arrow keys wrap, move focus, and skip disabled options", async ({ page, mount }) => {
  await mount(`<adi-segmented id="mode" value="a"><button value="a">First</button><button value="b" disabled>Unavailable</button><button value="c">Third</button></adi-segmented>`);
  await page.evaluate(() => {
    window.changes = [];
    document.getElementById("mode").addEventListener("change", event => window.changes.push(event.detail.value));
  });
  await page.locator("#mode button[value=a]").focus();
  await page.keyboard.press("ArrowRight");
  assert.deepEqual(await page.locator("#mode").evaluate(node => ({ value: node.value, focused: node.shadowRoot.activeElement?.getAttribute("value") })), { value: "c", focused: "c" });
  await page.keyboard.press("ArrowRight");
  assert.deepEqual(await page.locator("#mode").evaluate(node => ({ value: node.value, focused: node.shadowRoot.activeElement?.getAttribute("value") })), { value: "a", focused: "a" });
  await page.keyboard.press("ArrowLeft");
  assert.deepEqual(await page.locator("#mode").evaluate(node => ({ value: node.value, focused: node.shadowRoot.activeElement?.getAttribute("value") })), { value: "c", focused: "c" });
  assert.deepEqual(await page.evaluate(() => window.changes), ["c", "a", "c"]);
});

test("segmented options do not submit their enclosing form", async ({ page, mount }) => {
  await mount(`<form><adi-segmented id="mode" value="a"><button value="a">First</button><button value="b">Second</button></adi-segmented></form>`);
  await page.evaluate(() => {
    window.submissions = 0;
    document.querySelector("form").addEventListener("submit", event => { event.preventDefault(); window.submissions++; });
  });
  await page.locator("#mode button[value=b]").click();
  await page.locator("#mode button[value=b]").press("Enter");
  assert.equal(await page.evaluate(() => window.submissions), 0);
  assert.equal(await page.locator("#mode").evaluate(node => node.value), "b");
});

test("segmented arrows leave a lone enabled option selected without duplicate changes", async ({ page, mount }) => {
  await mount(`<adi-segmented id="mode" value="b"><button value="a" disabled>First</button><button value="b">Only available</button><button value="c" disabled>Third</button></adi-segmented>`);
  await page.evaluate(() => {
    window.changes = [];
    document.getElementById("mode").addEventListener("change", event => window.changes.push(event.detail.value));
  });
  await page.locator("#mode button[value=b]").focus();
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("ArrowLeft");
  assert.deepEqual(await page.locator("#mode").evaluate(node => ({
    value: node.value, focused: node.shadowRoot.activeElement?.getAttribute("value"), changes: window.changes,
  })), { value: "b", focused: "b", changes: [] });
});

test("buttons activate by pointer and keyboard without submitting their enclosing form", async ({ page, mount }) => {
  await mount(`<form><adi-button id="action">Run action</adi-button></form>`);
  await page.evaluate(() => {
    window.clicks = 0;
    window.submissions = 0;
    document.getElementById("action").addEventListener("click", () => window.clicks++);
    document.querySelector("form").addEventListener("submit", event => { event.preventDefault(); window.submissions++; });
  });
  await page.getByRole("button", { name: "Run action" }).click();
  await page.getByRole("button", { name: "Run action" }).press("Enter");
  assert.deepEqual(await page.evaluate(() => ({ clicks: window.clicks, submissions: window.submissions })), { clicks: 2, submissions: 0 });
});

test("disabled buttons suppress activation and respond when re-enabled", async ({ page, mount }) => {
  await mount(`<button id="before">Before</button><adi-button id="action" disabled>Run action</adi-button><button id="after">After</button>`);
  await page.evaluate(() => {
    window.clicks = 0;
    document.getElementById("action").addEventListener("click", () => window.clicks++);
  });
  const button = page.getByRole("button", { name: "Run action" });
  assert.equal(await button.isDisabled(), true);
  await button.click({ force: true });
  assert.equal(await page.evaluate(() => window.clicks), 0);
  await page.locator("#before").focus();
  await page.keyboard.press("Tab");
  assert.equal(await page.locator("#after").evaluate(node => document.activeElement === node), true);
  await page.locator("#action").evaluate(node => node.removeAttribute("disabled"));
  assert.equal(await button.isDisabled(), false);
  await button.click();
  assert.equal(await page.evaluate(() => window.clicks), 1);
});

test("disabled link buttons block pointer and keyboard navigation and leave the tab order", async ({ page, mount }) => {
  await mount(`<button id="before">Before</button><adi-button id="action" href="#destination" disabled>Open destination</adi-button><button id="after">After</button>`);
  await page.evaluate(() => {
    window.clicks = 0;
    document.getElementById("action").addEventListener("click", () => window.clicks++);
  });
  const link = page.getByRole("link", { name: "Open destination" });
  assert.equal(await link.getAttribute("aria-disabled"), "true");
  await link.click({ force: true });
  await link.press("Enter");
  assert.deepEqual(await page.evaluate(() => ({ clicks: window.clicks, hash: location.hash })), { clicks: 0, hash: "" });
  await page.locator("#before").focus();
  await page.keyboard.press("Tab");
  assert.equal(await page.locator("#after").evaluate(node => document.activeElement === node), true);

  await page.locator("#action").evaluate(node => node.removeAttribute("disabled"));
  assert.equal(await link.getAttribute("aria-disabled"), "false");
  await page.locator("#before").focus();
  await page.keyboard.press("Tab");
  assert.equal(await link.evaluate(node => node.getRootNode().activeElement === node), true);
  await page.keyboard.press("Enter");
  assert.deepEqual(await page.evaluate(() => ({ clicks: window.clicks, hash: location.hash })), { clicks: 1, hash: "#destination" });
});
