/**
 * Cloudflare Worker entrypoint. All routing lives in {@link handle}; this just adapts the
 * module-worker `fetch` signature to it. (For a Cloudflare Pages deployment, the same
 * `handle` is wired through `functions/[[path]].ts` instead.)
 */

import { handle } from "./router";
import type { Env } from "./types";

export default {
  fetch(request: Request, env: Env): Promise<Response> {
    return handle(request, env);
  },
};
