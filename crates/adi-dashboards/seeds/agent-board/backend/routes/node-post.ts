// POST /node/<machine>/api/agents/… — everything the chat does to a paired machine's conversation,
// written through (see `lib/node.ts`).

import { node } from "../lib/node.ts";

export const method = "POST";
export const path = "/node";

export default node;
