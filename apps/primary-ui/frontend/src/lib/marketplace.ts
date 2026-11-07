import { APP_API_BASE } from "./app-api";

export type MarketplaceMedia = { url: string; kind?: string; caption?: string | null; poster?: string | null };
export type MarketplaceElement = { kind: string; name: string; description?: string | null; id?: string | null; started?: boolean | null };
export type BundleInstall = { project?: string | null; project_name?: string | null; commit: string; outdated: boolean; missing_secrets: string[]; elements: MarketplaceElement[] };
export type MarketplaceApp = {
  marketplace: string; slug: string; name: string; description?: string | null; version?: string | null;
  repo: string; commit: string; branch?: string | null; icon?: string | null; readme?: string | null;
  keywords: string[]; gallery: MarketplaceMedia[]; elements: MarketplaceElement[];
  installs: { id: string; name: string; host?: string | null; started: boolean; outdated: boolean; commit: string }[];
  bundle?: { declared: number; elements: MarketplaceElement[]; installs: BundleInstall[] } | null;
};
export type MarketplaceState = { sources: { name: string; url: string; synced_at?: number | null; error?: string | null }[]; apps: MarketplaceApp[] };
export const marketplaceKey = ["marketplace-state"] as const;
export const marketplacePath = (app: Pick<MarketplaceApp, "marketplace" | "slug">) => `/marketplace/${encodeURIComponent(app.marketplace)}/${encodeURIComponent(app.slug)}`;

export async function marketplaceRequest<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
  const response = await fetch(`${APP_API_BASE}${path}`, {
    method: body === undefined ? "GET" : "POST", signal,
    ...(body === undefined ? {} : { headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }),
  });
  const data = await response.json();
  if (!response.ok) throw new Error(typeof data.error === "string" ? data.error : "The marketplace request failed. Try again.");
  return data as T;
}
export const loadMarketplace = ({ signal }: { signal: AbortSignal }) => marketplaceRequest<MarketplaceState>("/marketplace", undefined, signal);
export const elementSpec = (element: MarketplaceElement) => {
  const dirs: Record<string, string> = { agent: "agents", tool: "tools", dashboard: "dashboards", service: "services", embedding: "embeddings", trigger: "triggers" };
  const kind = dirs[element.kind] ?? element.kind;
  return kind === "project" ? "project" : `${kind}/${element.name}`;
};
