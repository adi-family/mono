/** The showcase relays app requests; the primary app talks to the native API directly. */
export const APP_API_BASE = import.meta.env?.VITE_APP_API_BASE?.replace(/\/+$/, "") || "/api/app";
export const IS_PRIMARY_APP = APP_API_BASE === "/api";

export function appApiBase(source?: string): string {
  if (source === undefined) return APP_API_BASE;
  // Paired node petnames are single lowercase DNS labels, never URL fragments.
  if (!/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(source)) {
    throw new Error("Choose a valid paired machine.");
  }
  return `${APP_API_BASE}/node/${encodeURIComponent(source)}/api`;
}

/** Keep native settings on the panel's own origin, including panels reached through the mesh. */
export function appHref(path: string): string {
  return `${IS_PRIMARY_APP ? "" : "http://app.adi"}${path}`;
}
