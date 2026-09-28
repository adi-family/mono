// POST /stop {name, run_id} — cut off the turn in flight; the conversation stays.

import { body, panel } from "../lib/panel.ts";

export const method = "POST";
export const path = "/stop";

export default async (req: Request) => {
  const { name, run_id } = await body(req);
  return panel("/api/agents/run/stop", { name, run_id });
};
