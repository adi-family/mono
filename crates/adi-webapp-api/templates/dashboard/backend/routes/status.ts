// Example route: GET /status — host facts the example frontend module renders.
//
// The shape every route file follows: default-export a handler, optionally export `method`
// and `path`. Without `path`, the file name is the route (`status.ts` -> `/status`).

export const method = "GET";
export const path = "/status";

export default function status(_req: Request, ctx: { dashboard: string }) {
  return Response.json({
    dashboard: ctx.dashboard,
    runtime: `bun ${Bun.version}`,
    platform: process.platform,
    uptimeSeconds: Math.round(process.uptime()),
    memoryMb: Math.round(process.memoryUsage().rss / 1024 / 1024),
    now: new Date().toISOString(),
  });
}
