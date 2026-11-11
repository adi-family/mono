import { appApiBase } from "./app-api";

export type MarketplaceMedia = { url: string; kind?: string; caption?: string | null; poster?: string | null };
export type MarketplaceElement = { kind: string; name: string; description?: string | null; id?: string | null; started?: boolean | null };
export type BundleInstall = { project?: string | null; project_name?: string | null; commit: string; outdated: boolean; missing_secrets: string[]; elements: MarketplaceElement[] };
export type MarketplaceApp = {
  marketplace: string; slug: string; name: string; description?: string | null; summary?: string | null; version?: string | null;
  repo: string; commit: string; branch?: string | null; icon?: string | null; readme?: string | null;
  keywords: string[]; gallery: MarketplaceMedia[]; elements: MarketplaceElement[];
  installs: { id: string; name: string; host?: string | null; started: boolean; outdated: boolean; commit: string }[];
  bundle?: { declared: number; elements: MarketplaceElement[]; installs: BundleInstall[] } | null;
};
export type MarketplaceState = { sources: { name: string; url: string; synced_at?: number | null; error?: string | null }[]; apps: MarketplaceApp[] };
/** A bundle's dashboard element lands in the Dashboards registry: archived until started, then answering on its host. */
export type DashboardRow = { id: string; host?: string | null; archived_at?: number | null };
// Every catalog read and write is one machine's; `source` is a paired node's petname, or this machine.
export const marketplaceKey = (source?: string) => ["marketplace-state", source ?? ""] as const;
export const marketplaceProjectsKey = (source?: string) => ["marketplace-projects", source ?? ""] as const;
export const marketplaceDashboardsKey = (source?: string) => ["marketplace-dashboards", source ?? ""] as const;
export const marketplacePath = (app: Pick<MarketplaceApp, "marketplace" | "slug">) => `/marketplace/${encodeURIComponent(app.marketplace)}/${encodeURIComponent(app.slug)}`;

export async function marketplaceRequest<T>(source: string | undefined, path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
  const response = await fetch(`${appApiBase(source)}${path}`, {
    method: body === undefined ? "GET" : "POST", signal,
    ...(body === undefined ? {} : { headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }),
  });
  const data = await response.json();
  if (!response.ok) throw new Error(typeof data.error === "string" ? data.error : "The marketplace request failed. Try again.");
  return data as T;
}
export const loadMarketplace = (source?: string) => ({ signal }: { signal: AbortSignal }) => marketplaceRequest<MarketplaceState>(source, "/marketplace", undefined, signal);
export const loadDashboards = (source?: string) => async ({ signal }: { signal: AbortSignal }) => (await marketplaceRequest<{ dashboards: DashboardRow[] }>(source, "/dashboards", undefined, signal)).dashboards ?? [];

/** The bundle elements whose ids name a dashboard on the machine — what the Dashboards registry has to be asked about. */
export const hasDashboardElements = (app: Pick<MarketplaceApp, "bundle">) => !!app.bundle?.installs.some((copy) => copy.elements.some((element) => element.kind === "dashboards" && !!element.id));

/** Where a started copy opens. An installed app's `.adi` host is its own machine's name for it;
 * from here a paired machine's app is reached through the mesh as `<name>.<machine>.n.adi`. */
export function installHref(host: string | null | undefined, source?: string): string | null {
  const name = host?.trim().toLowerCase();
  if (!name) return null;
  if (!source) return `http://${name}`;
  const service = name.endsWith(".adi") ? name.slice(0, -".adi".length) : "";
  return /^[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$/.test(service) ? `http://${service}.${source}.n.adi/` : null;
}

export const elementSpec = (element: MarketplaceElement) => {
  const dirs: Record<string, string> = { agent: "agents", tool: "tools", dashboard: "dashboards", service: "services", embedding: "embeddings", trigger: "triggers" };
  const kind = dirs[element.kind] ?? element.kind;
  return kind === "project" ? "project" : `${kind}/${element.name}`;
};
