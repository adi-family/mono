/**
 * The Worker entrypoint. All the logic lives in `router.ts` and `do.ts`; this just wires them
 * to the two things `wrangler.toml` binds: the default export's `fetch`, and the
 * `NodeConnection` class `durable_objects.bindings` names.
 */
import { handle } from "./router";
import type { Env } from "./types";

export { NodeConnection } from "./do";

export default {
  fetch(request: Request, env: Env): Promise<Response> {
    return handle(request, env);
  },
};
