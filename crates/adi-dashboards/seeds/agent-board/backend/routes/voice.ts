// GET /voice — the panel's speech engines, for the chat's microphone.

import { relay } from "../lib/panel.ts";

export const method = "GET";
export const path = "/voice";

export default (req: Request) => relay(req, "/api/voice");
