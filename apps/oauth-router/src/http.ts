/**
 * Tiny response constructors. Every response the router returns is uncacheable — it
 * carries a redirect to a provider, a redirect bearing a token, or an error — so
 * `Cache-Control: no-store` is baked in here rather than remembered at each call site.
 */

const NO_STORE = "no-store";

/** A 302 to `location`, optionally setting one `Set-Cookie`. */
export function redirectTo(location: string, setCookie?: string): Response {
  const headers = new Headers({ location, "cache-control": NO_STORE });
  if (setCookie) headers.append("set-cookie", setCookie);
  return new Response(null, { status: 302, headers });
}

/** A JSON body with the given status. */
export function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": NO_STORE },
  });
}

/** A plain-text error, optionally clearing a cookie on the way out. */
export function problem(status: number, message: string, setCookie?: string): Response {
  const headers = new Headers({
    "content-type": "text/plain; charset=utf-8",
    "cache-control": NO_STORE,
  });
  if (setCookie) headers.append("set-cookie", setCookie);
  return new Response(`${message}\n`, { status, headers });
}
