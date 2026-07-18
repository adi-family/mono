// Example module: renders the backend's /status, refreshing every few seconds.
//
// The shape every module file follows: default-export a function taking the context the shell
// builds. `ctx.panel(title)` returns an element to render into; `ctx.api` is pre-bound to the
// backend, so a module never needs to know its port.

interface Ctx {
  dashboard: string;
  api: { base: string | null; get(path: string): Promise<any> };
  panel(title?: string): HTMLElement;
}

export default async function status(ctx: Ctx) {
  const el = ctx.panel("Backend status");

  async function render() {
    try {
      const data = await ctx.api.get("/status");
      el.innerHTML = "";
      const dl = document.createElement("dl");
      dl.style.cssText =
        "display:grid;grid-template-columns:auto 1fr;gap:6px 16px;margin:0;font-size:13px";
      for (const [key, value] of Object.entries(data)) {
        const dt = document.createElement("dt");
        dt.textContent = key;
        dt.style.cssText = "color:var(--muted)";
        const dd = document.createElement("dd");
        dd.textContent = String(value);
        dd.style.cssText = "margin:0;font-family:ui-monospace,SFMono-Regular,monospace";
        dl.append(dt, dd);
      }
      el.append(dl);
    } catch (err) {
      el.textContent = `unavailable — ${err}`;
    }
  }

  await render();
  setInterval(render, 5000);
}
