// The greeting and the clock — the whole of this dashboard.
//
// It says the server's time, not the phone's, and the difference has to survive a reviewer holding
// a device whose clock is wrong. So the page asks the backend for `epochMs`, keeps the *offset*
// between that and its own clock, and renders `now + offset` — a smooth second-by-second tick that
// is still the machine's time. Re-syncing every ten seconds keeps it honest without asking a phone
// on a relayed connection to make a request per second.
//
// It also patches instead of re-rendering (`guides/dashboards.md`): the nodes are built once and
// only their text changes, so nothing the reader is touching is destroyed on a tick.

interface Ctx {
  dashboard: string;
  api: { base: string | null; get(path: string): Promise<any> };
  panel(title?: string): HTMLElement;
}

export default async function hello(ctx: Ctx) {
  const el = ctx.panel("Hello");

  const line = document.createElement("p");
  line.style.cssText = "margin:0;font-size:17px;line-height:1.6";
  const lead = document.createElement("span");
  lead.textContent = "Hello reviewer, server time is ";
  const clock = document.createElement("time");
  clock.style.cssText =
    "font-family:ui-monospace,SFMono-Regular,monospace;font-weight:600;white-space:nowrap";
  clock.textContent = "…";
  line.append(lead, clock);

  const note = document.createElement("p");
  note.style.cssText = "margin:14px 0 0;font-size:13px;color:var(--muted);line-height:1.5";
  note.textContent = "Asking the machine…";

  el.append(line, note);

  // Millis to add to this device's clock to get the server's.
  let offset = 0;
  let synced = false;

  async function sync() {
    try {
      const data = await ctx.api.get("/time");
      offset = data.epochMs - Date.now();
      synced = true;
      note.textContent = `Served live by ${data.host} (${data.timezone}), reached over the adi mesh — no port is open on either side.`;
    } catch (err) {
      synced = false;
      note.textContent = `Could not reach this machine's backend — ${err}`;
    }
  }

  function tick() {
    if (!synced) {
      clock.textContent = "unavailable";
      return;
    }
    const at = new Date(Date.now() + offset);
    clock.textContent = `${at.toISOString().slice(0, 19).replace("T", " ")} UTC`;
    clock.dateTime = at.toISOString();
  }

  await sync();
  tick();
  setInterval(tick, 1000);
  setInterval(sync, 10000);
}
