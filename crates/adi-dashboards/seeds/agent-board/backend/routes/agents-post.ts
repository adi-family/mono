// POST /agents/… — the panel's agent API, written through: everything `<adi-chat>` does to a
// conversation (peek, reply, start, stop, answer, unqueue, goals, attachments) at the same path the
// panel serves it on, so the widget's chat is the panel's chat with this origin in front of it.

import { relay } from "../lib/panel.ts";

export const method = "POST";
export const path = "/agents";

export default (req: Request, ctx: { params: string[] }) =>
  relay(req, ["/api/agents", ...ctx.params].join("/"));
