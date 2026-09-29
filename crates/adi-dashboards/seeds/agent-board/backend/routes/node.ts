// GET /node/<machine>/api/agents[/…] — a paired machine's agent list, runs and attachments, read
// through (see `lib/node.ts` for why nothing else on that machine is).

import { node } from "../lib/node.ts";

export const method = "GET";
export const path = "/node";

export default node;
