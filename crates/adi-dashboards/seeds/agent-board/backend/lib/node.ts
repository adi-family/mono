// A paired machine's agent API, through this machine's panel (`/api/node/<node>/…`, which dials
// the machine over the mesh with the credentials the panel holds). Only `/api/agents/…` is let
// through: that is all the chat drives on another machine, and the widget's origin is a routable
// hostname, so it must not become a door to the rest of a paired machine's panel.

import { relay } from "./panel.ts";

export function node(req: Request, ctx: { params: string[]; url: URL }): Promise<Response> | Response {
  const [machine, api, agents, ...rest] = ctx.params;
  if (!machine || api !== "api" || agents !== "agents") {
    return Response.json({ error: "not found" }, { status: 404 });
  }
  const path = ["/api/node", encodeURIComponent(decodeURIComponent(machine)), "api/agents", ...rest].join("/");
  return relay(req, `${path}${ctx.url.search}`);
}
