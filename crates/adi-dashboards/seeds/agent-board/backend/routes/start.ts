// POST /start {name, message} — begin a new conversation with an agent; answers its run_id.

import { body, panel } from "../lib/panel.ts";

export const method = "POST";
export const path = "/start";

export default async (req: Request) => {
  const { name, message } = await body(req);
  return panel("/api/agents/run", { name, message });
};
