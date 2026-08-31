// The storefront: three numbers, twelve hours, six orders.
//
// This is the dashboard the App Store screenshot is taken of, so it has one job beyond being
// true: it has to look like something worth carrying in a pocket. That means real structure —
// figures, a shape over time, a list with state — rather than a paragraph of text.
//
// It patches instead of re-rendering (`guides/dashboards.md`): the nodes are built once and only
// their text and heights change, so a poll never destroys what the reader is looking at.
//
// Colour is the ADI accent `#FA5019` and nothing else. Two accent spends, both carrying
// information: the bar for the current hour, and the "paid" state that still owes work.

interface Ctx {
  dashboard: string;
  api: { base: string | null; get(path: string): Promise<any> };
  panel(title?: string): HTMLElement;
}

const ACCENT = "#FA5019";

// The headline figure drops the cents. Three columns across a dashboard panel is about a hundred
// points each, and "$3,169.00" does not fit in that at any size a headline should be — it came out
// as "$3,17…" on both devices. A day's takings to the penny is not what that number is for.
const bigMoney = (cents: number) =>
  "$" + Math.round(cents / 100).toLocaleString("en-US");

const money = (cents: number) =>
  "$" + (cents / 100).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });

function el(tag: string, css: string, text?: string): HTMLElement {
  const node = document.createElement(tag);
  node.style.cssText = css;
  if (text !== undefined) node.textContent = text;
  return node;
}

export default async function shop(ctx: Ctx) {
  const root = ctx.panel("Demo Shop");

  // ---- the three figures ----------------------------------------------------------------
  const figures = el("div",
    "container-type:inline-size;display:grid;grid-template-columns:repeat(3,1fr);gap:18px;margin-bottom:26px");
  const stat = (label: string) => {
    const box = el("div", "min-width:0");
    const value = el("div",
      "font-size:clamp(17px,8.2cqw,28px);font-weight:650;letter-spacing:-.022em;line-height:1.1;font-variant-numeric:tabular-nums;white-space:nowrap;overflow:hidden;text-overflow:ellipsis", "—");
    const cap = el("div",
      "margin-top:6px;font-size:11px;letter-spacing:.04em;text-transform:uppercase;color:var(--muted)", label);
    box.append(value, cap);
    figures.append(box);
    return value;
  };
  const ordersEl = stat("Orders today");
  const revenueEl = stat("Revenue");
  const awaitingEl = stat("To fulfil");

  // ---- twelve hours ---------------------------------------------------------------------
  const chart = el("div", "display:flex;align-items:flex-end;gap:6px;height:76px;margin-bottom:8px");
  const bars: HTMLElement[] = [];
  for (let i = 0; i < 12; i++) {
    const bar = el("div",
      "flex:1;min-width:0;border-radius:4px 4px 2px 2px;background:color-mix(in oklab, currentColor 16%, transparent);height:6px;transition:height .3s ease-out");
    bars.push(bar);
    chart.append(bar);
  }
  const chartCap = el("div",
    "font-size:11px;letter-spacing:.04em;text-transform:uppercase;color:var(--muted);margin-bottom:26px",
    "Orders, last 12 hours");

  // ---- the orders -----------------------------------------------------------------------
  const list = el("div", "display:flex;flex-direction:column;gap:0");
  const rows: {
    row: HTMLElement; id: HTMLElement; item: HTMLElement; when: HTMLElement;
    amount: HTMLElement; pill: HTMLElement;
  }[] = [];
  for (let i = 0; i < 6; i++) {
    const row = el("div",
      "display:flex;align-items:center;gap:14px;padding:11px 0;border-top:1px solid var(--border)");
    const left = el("div", "min-width:0;flex:1");
    const id = el("div", "font-size:13px;font-weight:600;font-variant-numeric:tabular-nums", "—");
    const item = el("div", "margin-top:2px;font-size:12.5px;color:var(--muted);white-space:nowrap;overflow:hidden;text-overflow:ellipsis", "—");
    left.append(id, item);
    const when = el("div", "font-size:12px;color:var(--muted);white-space:nowrap", "");
    const amount = el("div", "font-size:13.5px;font-weight:600;font-variant-numeric:tabular-nums;white-space:nowrap", "");
    const pill = el("div",
      "font-size:11px;font-weight:600;padding:3px 9px;border-radius:999px;white-space:nowrap", "");
    row.append(left, when, amount, pill);
    list.append(row);
    rows.push({ row, id, item, when, amount, pill });
  }

  const note = el("div",
    "margin-top:18px;font-size:11.5px;color:var(--muted);line-height:1.5",
    "Demo data, generated on this machine.");

  root.append(figures, chart, chartCap, list, note);

  function paint(d: any) {
    ordersEl.textContent = String(d.orders);
    revenueEl.textContent = bigMoney(d.revenueCents);
    awaitingEl.textContent = String(d.awaiting);

    const peak = Math.max(...d.hours.map((h: any) => h.orders), 1);
    d.hours.forEach((h: any, i: number) => {
      bars[i].style.height = `${Math.max(6, Math.round((h.orders / peak) * 76))}px`;
      // The current hour is the one the eye should land on, so it is the one that gets the accent.
      bars[i].style.background =
        i === d.hours.length - 1 ? ACCENT : "color-mix(in oklab, currentColor 16%, transparent)";
    });

    d.recent.forEach((o: any, i: number) => {
      const r = rows[i];
      if (!r) return;
      r.id.textContent = o.id;
      r.item.textContent = `${o.qty} × ${o.item}`;
      r.when.textContent = o.minutesAgo < 1 ? "just now" : `${o.minutesAgo} min`;
      r.amount.textContent = money(o.cents);
      r.pill.textContent = o.stage;
      // Accent for the state that still owes work; the rest are quiet.
      if (o.stage === "paid") {
        r.pill.style.color = ACCENT;
        r.pill.style.background = "color-mix(in oklab, " + ACCENT + " 16%, transparent)";
      } else {
        r.pill.style.color = "var(--muted)";
        r.pill.style.background = "color-mix(in oklab, currentColor 10%, transparent)";
      }
    });

    note.textContent = `Demo data, generated on ${d.servedBy} and reached over the adi mesh — no port is open on either side.`;
  }

  async function refresh() {
    try {
      paint(await ctx.api.get("/shop"));
    } catch (err) {
      note.textContent = `Could not reach this machine's backend — ${err}`;
    }
  }

  await refresh();
  setInterval(refresh, 15000);
}
