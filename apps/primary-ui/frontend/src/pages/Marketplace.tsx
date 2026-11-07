import { useEffect, useState, type ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowUpRight, Bot, LayoutGrid, LoaderCircle, Package, Search, Wrench, X } from "lucide-react";
import { Button, Card, cn } from "@design-system/ui";
import { Link } from "react-router-dom";
import { loadMarketplace, marketplaceKey, marketplacePath, marketplaceRequest, type MarketplaceApp, type MarketplaceState } from "../lib/marketplace";

const categories = ["All", "Agents", "Tools", "Dashboards", "Services"] as const;
type Category = typeof categories[number];
type Listing = MarketplaceApp;
const kinds: Record<Category, string | undefined> = { All: undefined, Agents: "agent", Tools: "tool", Dashboards: "dashboard", Services: "service" };

export function imageUrl(value: unknown): string | undefined {
  if (typeof value !== "string") return;
  const url = value.trim();
  return /^(https:\/\/|data:image\/)/i.test(url) ? url : undefined;
}

export function CatalogImage({ src, alt, slot, className, fallback }: { src: string; alt: string; slot: string; className: string; fallback?: ReactNode }) {
  const [failed, setFailed] = useState(false);
  return failed ? fallback ?? <div className={cn("text-muted-foreground flex items-center justify-center text-xs", className)}>Image unavailable</div> : <img src={src} alt={alt} data-slot={slot} className={className} decoding="async" referrerPolicy="no-referrer" onError={() => setFailed(true)} />;
}


function ListingCard({ item }: { item: Listing }) {
  const href = marketplacePath(item);
  const Icon = item.elements?.some((element) => element.kind === "agent") ? Bot : item.elements?.some((element) => element.kind === "tool") ? Wrench : LayoutGrid;
  const icon = imageUrl(item.icon);
  const gallery = Array.isArray(item.gallery) ? item.gallery.flatMap((image) => {
    const video = image?.kind === "video";
    const url = video ? (typeof image.url === "string" && /^(https:\/\/|data:video\/)/i.test(image.url.trim()) ? image.url.trim() : undefined) : imageUrl(image?.url);
    return url ? [{ url, video, poster: imageUrl(image.poster), caption: typeof image.caption === "string" ? image.caption : undefined }] : [];
  }) : [];
  const preview = gallery[0]?.video ? gallery[0].poster : gallery[0]?.url;
  const fallbackIcon = <Icon className="size-5" aria-hidden="true" />;
  // The card is a summary; gallery, elements and install details live on the package page.
  return <Card className="flex flex-col gap-0 overflow-hidden p-0 shadow-none">
    {preview && <Link to={href} aria-label={`Open ${item.name}`} className="focus-visible:ring-ring block focus-visible:ring-2 focus-visible:ring-inset"><CatalogImage key={preview} src={preview} alt={gallery[0].caption ?? `${item.name} preview`} slot="marketplace-preview" className="bg-muted aspect-video w-full border-b object-contain" /></Link>}
    <div className="flex flex-1 flex-col p-5">
      <div className="mb-5 flex items-center justify-between gap-3">
        <span className="bg-muted text-foreground flex size-11 items-center justify-center overflow-hidden rounded-xl">{icon ? <CatalogImage key={icon} src={icon} alt="" slot="marketplace-icon" className="size-11 object-contain" fallback={fallbackIcon} /> : fallbackIcon}</span>
        <span className="text-muted-foreground text-xs">{item.version ? `v${item.version}` : item.marketplace}</span>
      </div>
      <h3 className="font-semibold tracking-tight"><Link to={href} className="focus-visible:ring-ring rounded-sm hover:underline focus-visible:ring-2">{item.name}</Link></h3>
      <p className="text-muted-foreground mt-2 flex-1 text-sm leading-relaxed">{item.description}</p>
      <div className="mt-5 flex flex-wrap gap-1.5">{[...new Set(item.elements?.map((element) => element.kind) ?? ["app"])].map((kind) => <span key={kind} className="bg-muted text-muted-foreground rounded-md px-2 py-1 text-xs capitalize">{kind}</span>)}</div>
      <Button asChild variant="outline" className="mt-5 w-full" data-slot="marketplace-package-link"><Link to={href}>View package <ArrowUpRight className="size-3.5" aria-hidden="true" /></Link></Button>
    </div>
  </Card>;
}

export function Marketplace() {
  const cache = useQueryClient();
  const [search, setSearch] = useState("");
  const [category, setCategory] = useState<Category>("All");
  const catalog = useQuery({ queryKey: marketplaceKey, queryFn: loadMarketplace, staleTime: 30000, retry: false });
  const sync = useMutation({ mutationFn: () => marketplaceRequest<{ state: MarketplaceState; message: string }>("/marketplace/sync", null), onSuccess: async (result) => { await cache.cancelQueries({ queryKey: marketplaceKey }); cache.setQueryData(marketplaceKey, result.state); } });
  useEffect(() => { document.title = "Marketplace — ADI"; }, []);
  const items = (catalog.data?.apps ?? []).filter((item) => (!kinds[category] || item.elements?.some((element) => element.kind.replace(/s$/, "") === kinds[category])) && `${item.name} ${item.description} ${item.elements?.map((element) => element.name).join(" ") ?? ""}`.toLowerCase().includes(search.trim().toLowerCase()));
  return <main aria-label="Marketplace" className="mx-auto w-full max-w-7xl py-5 sm:px-4 sm:py-8 lg:px-6 lg:py-10">
    <header className="mb-8"><p className="text-muted-foreground mb-3 flex items-center gap-2 text-xs font-medium"><Package className="size-4" aria-hidden="true" /> ADI COLLECTION</p><h2 className="text-3xl font-semibold tracking-tight">Make ADI your own.</h2><p className="text-muted-foreground mt-3 max-w-xl text-sm leading-relaxed">Discover agents, tools, and apps for your workspace. Pick a package, explore what’s inside, and install it on your machine.</p></header>
    <div className="mb-6 flex flex-wrap items-center justify-between gap-4">
      <div role="group" aria-label="Package category" className="flex flex-wrap gap-1">{categories.map((value) => <Button key={value} variant="ghost" size="sm" aria-pressed={category === value} className={cn(category === value && "bg-muted text-foreground")} onClick={() => setCategory(value)}>{value}</Button>)}</div>
      <div className="focus-within:ring-ring flex w-full items-center gap-2 rounded-lg border px-3 focus-within:ring-2 sm:w-64"><Search className="text-muted-foreground size-4" aria-hidden="true" /><input aria-label="Search marketplace" placeholder="Search marketplace…" value={search} onChange={(event) => setSearch(event.target.value)} className="placeholder:text-muted-foreground h-9 w-full min-w-0 bg-transparent text-sm outline-none" />{search && <button aria-label="Clear search" className="focus-visible:ring-ring rounded-sm focus-visible:ring-2" onClick={() => setSearch("")}><X className="text-muted-foreground size-4" /></button>}</div>
    </div>
    <div className="mb-4 flex flex-wrap items-center justify-between gap-3 text-xs"><p className="text-muted-foreground">Sources: {catalog.data?.sources.map((source) => source.name).join(", ") || "None configured"}</p><Button variant="outline" size="sm" disabled={sync.isPending} disabledReason="The marketplace catalog is being refreshed." onClick={() => sync.mutate()}>{sync.isPending ? "Refreshing…" : "Refresh catalog"}</Button></div>
    {sync.isError && <p role="alert" className="text-destructive mb-4 text-sm">{sync.error.message}</p>}
    {sync.isSuccess && <p role="status" className="text-muted-foreground mb-4 text-sm whitespace-pre-wrap">{sync.data.message}</p>}
    {catalog.data?.sources.filter((source) => source.error).map((source) => <p key={source.name} role="alert" className="text-destructive mb-4 text-sm">{source.name}: {source.error}. Showing the last cached catalog.</p>)}
    {catalog.isPending ? <p role="status" className="text-muted-foreground flex items-center gap-2 py-12 text-sm"><LoaderCircle className="size-4 motion-safe:animate-spin" /> Loading packages…</p> : catalog.isError ? <div role="alert" className="rounded-xl border p-6"><p>Couldn’t load the marketplace.</p><p className="text-muted-foreground mt-1 text-sm">Check your connection and try again.</p><Button variant="outline" className="mt-4" onClick={() => void catalog.refetch()}>Try again</Button></div> : <><p role="status" className="text-muted-foreground mb-4 text-xs">{items.length} {items.length === 1 ? "package" : "packages"}</p>{items.length ? <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">{items.map((item) => <ListingCard key={`${item.marketplace}/${item.slug}`} item={item} />)}</div> : <div className="rounded-xl border border-dashed py-16 text-center"><Search className="text-muted-foreground mx-auto mb-4 size-6" /><h3 className="font-medium">No packages found</h3><p className="text-muted-foreground mt-2 text-sm">Try another search or category.</p><Button variant="outline" className="mt-5" onClick={() => { setSearch(""); setCategory("All"); }}>Reset filters</Button></div>}</>}
  </main>;
}
