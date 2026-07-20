/**
 * Cloudflare Pages entrypoint (alternative to the standalone Worker in `src/index.ts`).
 *
 * A catch-all Pages Function so the router owns every path when this project is deployed as
 * a Pages project instead of a Worker. Same {@link handle}, different wrapper — pick one
 * deploy target; you don't need both.
 */

import { handle } from "../src/router";
import type { Env } from "../src/types";

export const onRequest = (context: { request: Request; env: Env }): Promise<Response> =>
  handle(context.request, context.env);
