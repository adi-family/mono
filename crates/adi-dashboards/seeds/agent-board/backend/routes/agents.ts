// GET /agents — this machine's agents, as the panel lists them.

import { panel } from "../lib/panel.ts";

export const method = "GET";
export const path = "/agents";

export default () => panel("/api/agents");
