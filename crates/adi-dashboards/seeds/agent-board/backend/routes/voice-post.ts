// POST /voice/transcribe?engine=… — a dictated clip, passed to the panel as the raw bytes it is.

import { relay } from "../lib/panel.ts";

export const method = "POST";
export const path = "/voice";

export default (req: Request, ctx: { params: string[]; url: URL }) =>
  relay(req, ["/api/voice", ...ctx.params].join("/") + ctx.url.search);
