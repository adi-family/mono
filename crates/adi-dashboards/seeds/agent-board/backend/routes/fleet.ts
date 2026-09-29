// GET /fleet/nodes — the paired machines, read through, so the widget's chat can list their agents
// beside this machine's (`<adi-chat>` asks for it to fill its picker). Only that one read: the
// rest of `/fleet` changes the fleet, which the chat never does.

import { relay } from "../lib/panel.ts";

export const method = "GET";
export const path = "/fleet";

export default (req: Request, ctx: { params: string[] }) =>
  ctx.params.join("/") === "nodes"
    ? relay(req, "/api/fleet/nodes")
    : Response.json({ error: "not found" }, { status: 404 });
