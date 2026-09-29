// GET /agents[/…] — the panel's agent API, read through: the agent list, and an attached file by
// id (`/agents/attachment/<id>`) for the chat to draw.

import { relay } from "../lib/panel.ts";

export const method = "GET";
export const path = "/agents";

export default (req: Request, ctx: { params: string[] }) =>
  relay(req, ["/api/agents", ...ctx.params].join("/"));
