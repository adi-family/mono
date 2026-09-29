// GET /elements/<file> — the panel's own custom elements (`<adi-chat>` and what it is built
// from), fetched from the panel rather than copied: the widget loads the same files the panel
// does, so the two chats cannot drift. Their relative imports resolve back through this route.

import { element } from "../lib/panel.ts";

export const method = "GET";
export const path = "/elements";

export default (_req: Request, ctx: { params: string[] }) => {
  const file = ctx.params.join("/");
  if (!file || file.split("/").some((part) => part.startsWith("."))) {
    return new Response("not found", { status: 404 });
  }
  return element(file);
};
