// This machine's control panel — where agents live, run and keep their conversations.
//
// The board owns no state of its own: every agent, run and transcript is the panel's, read and
// driven through its API. Server-side, and not from the page, because the panel answers `/api`
// only to its own origin (a page on agent-board.adi is refused) or to a caller with no Origin at
// all — which is what a request from this process is.

/** Where the panel listens: loopback first, since a board depends on its own machine and not on
 *  DNS; `app.adi` only if that is not answering. `ADI_APP_URL` overrides both. */
const PANEL_URLS = [process.env.ADI_APP_URL?.trim(), "http://127.0.0.1:8000", "http://app.adi"]
  .filter(Boolean)
  .filter((url, i, all) => all.indexOf(url) === i) as string[];

/** The last address that answered, so the list is walked once rather than on every request. */
let base: string | null = null;

/** The panel's answer to `path` (a POST when there is a `payload`), passed through with its own
 *  status — a 404 for an agent that does not exist is the panel's fact to report, not ours. A
 *  panel that does not answer at all is a 503. */
export async function panel(path: string, payload?: unknown): Promise<Response> {
  let lastErr = "";
  for (const url of base ? [base] : PANEL_URLS) {
    try {
      const res = await fetch(`${url}${path}`, {
        method: payload === undefined ? "GET" : "POST",
        headers: payload === undefined ? {} : { "content-type": "application/json" },
        body: payload === undefined ? undefined : JSON.stringify(payload),
        signal: AbortSignal.timeout(20_000),
      });
      base = url;
      return new Response(await res.text(), {
        status: res.status,
        headers: { "content-type": "application/json" },
      });
    } catch (err) {
      lastErr = err instanceof Error ? err.message : String(err);
      base = null;
    }
  }
  return Response.json({ error: `the control panel is not answering: ${lastErr}` }, { status: 503 });
}

/** The request's JSON body, or `{}` — the panel says what is missing better than we could. */
export async function body(req: Request): Promise<Record<string, unknown>> {
  try {
    return (await req.json()) as Record<string, unknown>;
  } catch {
    return {};
  }
}
