// POST /runs {name} — one agent's conversations, as the panel lists them.

import { body, panel } from "../lib/panel.ts";

export const method = "POST";
export const path = "/runs";

export default async (req: Request) => panel("/api/agents/runs", { name: (await body(req)).name });
