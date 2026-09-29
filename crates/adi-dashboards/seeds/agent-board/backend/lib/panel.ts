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

/** The request passed through to the panel's own `path` as it came: method, body bytes and the
 *  headers that say what the body is — which is what an attachment upload (raw bytes, its name in
 *  `x-adi-filename`) and a dictated clip need, where `panel()` would have re-encoded them as JSON.
 *  Headers a browser sets (Origin, Cookie) stay behind, for the reason given at the top. */
export async function relay(req: Request, path: string): Promise<Response> {
  const headers: Record<string, string> = {};
  for (const name of ["content-type", "x-adi-filename"]) {
    const value = req.headers.get(name);
    if (value) headers[name] = value;
  }
  const body = req.method === "GET" || req.method === "HEAD" ? undefined : await req.arrayBuffer();
  let lastErr = "";
  for (const url of base ? [base] : PANEL_URLS) {
    try {
      const res = await fetch(`${url}${path}`, {
        method: req.method,
        headers,
        body,
        signal: AbortSignal.timeout(120_000),
      });
      base = url;
      return new Response(res.body, {
        status: res.status,
        headers: { "content-type": res.headers.get("content-type") ?? "application/json" },
      });
    } catch (err) {
      lastErr = err instanceof Error ? err.message : String(err);
      base = null;
    }
  }
  return Response.json({ error: `the control panel is not answering: ${lastErr}` }, { status: 503 });
}

/** Where the chat's own files come from (`design/elements`, served by every panel at
 *  `/elements/`): the panels above, then this machine's dev panel — a released panel older than
 *  the chat element answers `/elements/chat.js` with its HTML fallback, so only a JavaScript (or
 *  CSS) answer counts. `ADI_ELEMENTS_URL` overrides the lot. */
const ELEMENT_URLS = [process.env.ADI_ELEMENTS_URL?.trim(), ...PANEL_URLS, "http://127.0.0.1:9080"]
  .filter(Boolean)
  .filter((url, i, all) => all.indexOf(url) === i) as string[];

/** The panel the element files are taken from — one for the whole set. Mixing two is a module
 *  graph with an old `adi-elements.js` over a new `chat.js`, which registers nothing. Chosen as the
 *  first that serves `chat.js` as JavaScript, and chosen again whenever it stops answering. */
let elementsFrom: string | null = null;

async function elementSource(): Promise<string | null> {
  if (elementsFrom) return elementsFrom;
  for (const url of ELEMENT_URLS) {
    try {
      const res = await fetch(`${url}/elements/chat.js`, { signal: AbortSignal.timeout(5_000) });
      if (res.ok && /javascript/.test(res.headers.get("content-type") ?? "")) {
        elementsFrom = url;
        return url;
      }
    } catch {
      // Not answering — the next one may.
    }
  }
  return null;
}

/** One of the panel's element files, e.g. `chat.js` — the same file the panel itself loads. */
export async function element(file: string): Promise<Response> {
  const from = await elementSource();
  if (from) {
    try {
      const res = await fetch(`${from}/elements/${file}`, { signal: AbortSignal.timeout(10_000) });
      if (res.ok) {
        return new Response(res.body, {
          headers: { "content-type": res.headers.get("content-type") ?? "text/javascript", "cache-control": "no-store" },
        });
      }
    } catch {
      elementsFrom = null;
    }
  }
  return new Response(`/* no panel serves elements/${file} */`, { status: 404 });
}
