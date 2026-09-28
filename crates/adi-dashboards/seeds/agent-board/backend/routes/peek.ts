// POST /peek {name, run_id} — one conversation: every turn so far, the answer in flight included.

import { body, panel } from "../lib/panel.ts";

export const method = "POST";
export const path = "/peek";

export default async (req: Request) => {
  const { name, run_id } = await body(req);
  return panel("/api/agents/run/peek", { name, run_id });
};
