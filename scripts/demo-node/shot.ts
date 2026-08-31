// Screenshot a page that sits behind Basic auth, by injecting the header over CDP.
//
// The obvious `--screenshot http://user:pass@host/` does NOT work: Chrome loads the document but
// then refuses every same-origin `fetch` from it with "Request cannot be constructed from a URL
// that includes credentials", so the page renders with its data calls failing and the screenshot
// shows a bug that is not there. Setting the header applies to the document AND its subresources,
// which is the whole difference.
//
//   bun /tmp/dash/shot.ts <url> <basic-auth-user:pass> <out.png> [widthxheight] [mobile]

const [url, creds, out, size = "900x620", mobile = ""] = Bun.argv.slice(2);
const [width, height] = size.split("x").map(Number);
const PORT = 9788;

const chrome = Bun.spawn(
  [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "--headless=new",
    "--disable-gpu",
    `--remote-debugging-port=${PORT}`,
    "--no-first-run",
    `--user-data-dir=/tmp/dash/chrome-profile`,
    "about:blank",
  ],
  { stdout: "ignore", stderr: "ignore" },
);

async function target() {
  for (let i = 0; i < 60; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
      const page = list.find((t: any) => t.type === "page");
      if (page?.webSocketDebuggerUrl) return page.webSocketDebuggerUrl;
    } catch {}
    await Bun.sleep(250);
  }
  throw new Error("chrome never came up");
}

const ws = new WebSocket(await target());
await new Promise((r) => (ws.onopen = r));

let id = 0;
const pending = new Map<number, (v: any) => void>();
ws.onmessage = (e) => {
  const msg = JSON.parse(String(e.data));
  if (msg.id && pending.has(msg.id)) pending.get(msg.id)!(msg.result);
};
const send = (method: string, params: any = {}) =>
  new Promise<any>((resolve) => {
    const n = ++id;
    pending.set(n, resolve);
    ws.send(JSON.stringify({ id: n, method, params }));
  });

await send("Network.enable");
await send("Page.enable");
await send("Network.setExtraHTTPHeaders", {
  headers: { Authorization: `Basic ${Buffer.from(creds).toString("base64")}` },
});
await send("Emulation.setDeviceMetricsOverride", {
  width,
  height,
  deviceScaleFactor: mobile ? 3 : 2,
  mobile: Boolean(mobile),
});

await send("Page.navigate", { url });
// The clock syncs on load and ticks after; a couple of seconds is enough to prove it arrived.
await Bun.sleep(4000);

const shot = await send("Page.captureScreenshot", {
  format: "png",
  captureBeyondViewport: true,
});
await Bun.write(out, Buffer.from(shot.data, "base64"));

// What the panel actually says, so a green screenshot cannot come from an empty page.
const text = await send("Runtime.evaluate", {
  expression: "document.querySelector('main')?.innerText ?? '(no main)'",
  returnByValue: true,
});
console.log("--- rendered text ---");
console.log(text.result.value);

ws.close();
chrome.kill();
