/**
 * The Cloudflare Pages entrypoint — the whole router, as a catch-all Pages Function.
 *
 * `public/_routes.json` includes `/*`, so every request reaches this Function rather than the
 * static root, and `handle` owns the routing. All the logic lives in `../src/router`; this is
 * only the adapter from Pages' `onRequest` context to it.
 */

import { handle } from "../src/router";
import type { Env } from "../src/types";

export const onRequest = (context: { request: Request; env: Env }): Promise<Response> =>
  handle(context.request, context.env);
