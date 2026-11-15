import { appApiBase } from "./app-api";

/*
 * What every settings section shares: one request helper for a machine's native API, and the
 * machine's project list. Every read and write is one machine's; `source` is a paired node's
 * petname, or `undefined` for this machine.
 */

export type SettingsProject = { id: string; name: string; archived_at?: number | null };

export const settingsProjectsKey = (source?: string) => ["settings-projects", source ?? ""] as const;

export async function settingsRequest<T>(source: string | undefined, path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
  const response = await fetch(`${appApiBase(source)}${path}`, {
    method: body === undefined ? "GET" : "POST", signal,
    ...(body === undefined ? {} : { headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }),
  });
  let data: unknown;
  try { data = await response.json(); } catch { data = null; }
  const error = data && typeof data === "object" && typeof (data as { error?: unknown }).error === "string" ? (data as { error: string }).error : null;
  if (!response.ok || data === null) throw new Error(error ?? "The settings request failed. Try again.");
  return data as T;
}

/** The machine's projects that are still live — the scopes a setting can be filed under. */
export const loadSettingsProjects = (source?: string) => async ({ signal }: { signal: AbortSignal }) =>
  ((await settingsRequest<{ projects?: SettingsProject[] }>(source, "/projects", undefined, signal)).projects ?? []).filter((project) => !project.archived_at);
