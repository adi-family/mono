// POST /reply {name, run_id, message} — say something into a conversation. The panel starts the
// next turn, or queues it behind the answer still in flight.

import { body, panel } from "../lib/panel.ts";

export const method = "POST";
export const path = "/reply";

export default async (req: Request) => {
  const { name, run_id, message } = await body(req);
  return panel("/api/agents/run/reply", { name, run_id, message });
};
