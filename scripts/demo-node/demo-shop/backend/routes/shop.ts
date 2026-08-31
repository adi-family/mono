// GET /shop — a storefront's day, as this machine sees it.
//
// Every number here is generated on the node, from the node's clock, and it MOVES: the point of
// the screenshot this dashboard exists for is that a phone is looking at a remote machine, and a
// still life proves nothing. The order times are relative to now, the counters advance through the
// day, and the "live" row changes on every poll.
//
// It is demo data and says so on the page. Nothing here claims to be a real shop's revenue.

import { hostname } from "node:os";

export const method = "GET";
export const path = "/shop";

/** A deterministic pseudo-random in [0,1) from an integer seed — same minute, same shop. */
function rand(seed: number): number {
  const x = Math.sin(seed * 12.9898) * 43758.5453;
  return x - Math.floor(x);
}

const ITEMS = [
  ["Filter coffee, 1kg", 1800],
  ["Espresso blend, 500g", 1400],
  ["Chemex filters ×100", 900],
  ["Hand grinder", 6400],
  ["Cold brew kit", 3200],
  ["Decaf, 500g", 1500],
  ["Tasting flight", 2400],
  ["Gift card", 5000],
] as const;

const STAGES = ["paid", "packing", "shipped"] as const;

export default function shop(_req: Request, _ctx: { dashboard: string }) {
  const now = new Date();
  // The day advances: a shop that has been open eight hours has done more than one at opening.
  const minutes = now.getUTCHours() * 60 + now.getUTCMinutes();
  const openFor = Math.max(minutes - 8 * 60, 0);          // trading starts at 08:00 UTC
  const orders = Math.floor(openFor / 7) + 3;

  let revenue = 0;
  const recent = [];
  for (let i = 0; i < 6; i++) {
    const seed = Math.floor(now.getTime() / 60000) - i * 7;
    const [item, price] = ITEMS[Math.floor(rand(seed) * ITEMS.length)];
    const qty = 1 + Math.floor(rand(seed + 1) * 3);
    recent.push({
      id: `#${(10248 + orders - i).toString()}`,
      item,
      qty,
      cents: price * qty,
      stage: STAGES[Math.min(i, STAGES.length - 1)],
      minutesAgo: i * 7 + Math.floor(rand(seed + 2) * 5),
    });
  }
  for (let i = 0; i < orders; i++) {
    const [, price] = ITEMS[Math.floor(rand(i + 1) * ITEMS.length)];
    revenue += price * (1 + Math.floor(rand(i + 2) * 3));
  }

  // Twelve hourly buckets for the bar row, oldest first, ending on the current hour.
  const hours = Array.from({ length: 12 }, (_, i) => {
    const hour = (now.getUTCHours() - 11 + i + 24) % 24;
    return { hour, orders: Math.round(2 + rand(hour * 3 + 1) * 9) };
  });

  return Response.json({
    servedBy: hostname(),
    at: now.toISOString(),
    orders,
    revenueCents: revenue,
    awaiting: recent.filter((o) => o.stage !== "shipped").length,
    recent,
    hours,
  });
}
