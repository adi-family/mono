// GET /time — the demo node's own clock.
//
// This dashboard exists for App Review (`apps/ios/APPSTORE.md` §2.2): a reviewer pairs a phone
// with this machine and needs to see something that is obviously *live* and obviously coming from
// the machine rather than from the phone. A clock is the smallest thing that proves both.

import { hostname } from "node:os";

export const method = "GET";
export const path = "/time";

export default function time(_req: Request, _ctx: { dashboard: string }) {
  const now = new Date();
  return Response.json({
    // Epoch millis, because the page corrects its own clock against this rather than repainting
    // from a string — see the frontend module for why.
    epochMs: now.getTime(),
    iso: now.toISOString(),
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone ?? "UTC",
    host: hostname(),
  });
}
