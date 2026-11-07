import { useEffect, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowUpRight, Bot, Check, Copy, LayoutGrid, LoaderCircle, Package, Search, Wrench, X } from "lucide-react";
import { Button, Card, cn } from "@design-system/ui";

const catalogUrl = "https://raw.githubusercontent.com/adi-family/marketplace/main/apps/marketplace.json";
const categories = ["All", "Agents", "Tools", "Dashboards", "Services"] as const;
type Category = typeof categories[number];
type Listing = { slug: string; name: string; description: string; version: string; repo: string; icon?: string; gallery?: { url: string; caption?: string; kind?: string; poster?: string }[]; elements?: { kind: string; name: string; description?: string }[] };
const kinds: Record<Category, string | undefined> = { All: undefined, Agents: "agent", Tools: "tool", Dashboards: "dashboard", Services: "service" };

function imageUrl(value: unknown): string | undefined {
  if (typeof value !== "string") return;
  const url = value.trim();
  return /^(https:\/\/|data:image\/)/i.test(url) ? url : undefined;
}

function CatalogImage({ src, alt, slot, className, fallback }: { src: string; alt: string; slot: string; className: string; fallback?: ReactNode }) {
  const [failed, setFailed] = useState(false);
  return failed ? fallback ?? <div className={cn("text-muted-foreground flex items-center justify-center text-xs", className)}>Image unavailable</div> : <img src={src} alt={alt} data-slot={slot} className={className} decoding="async" referrerPolicy="no-referrer" onError={() => setFailed(true)} />;
}

async function loadCatalog(signal: AbortSignal): Promise<Listing[]> {
  const response = await fetch(catalogUrl, { signal });
  if (!response.ok) throw new Error("The marketplace catalog could not be loaded.");
  const data = await response.json();
  if (!Array.isArray(data.apps)) throw new Error("The marketplace returned an invalid catalog.");
  return data.apps.filter((item: Listing) => item && typeof item.slug === "string" && /^[a-z0-9-]+$/.test(item.slug) && typeof item.name === "string" && typeof item.description === "string" && typeof item.repo === "string" && item.repo.startsWith("https://github.com/") && (item.elements === undefined || (Array.isArray(item.elements) && item.elements.every((element) => element && typeof element.kind === "string" && typeof element.name === "string"))));
}

function ListingCard({ item }: { item: Listing }) {
  const [expanded, setExpanded] = useState(false);
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState(false);
  const command = `adi-mono marketplace install adi/${item.slug}`;
  const Icon = item.elements?.some((element) => element.kind === "agent") ? Bot : item.elements?.some((element) => element.kind === "tool") ? Wrench : LayoutGrid;
  const icon = imageUrl(item.icon);
  const gallery = Array.isArray(item.gallery) ? item.gallery.flatMap((image) => {
    const video = image?.kind === "video";
    const url = video ? (typeof image.url === "string" && /^(https:\/\/|data:video\/)/i.test(image.url.trim()) ? image.url.trim() : undefined) : imageUrl(image?.url);
    return url ? [{ url, video, poster: imageUrl(image.poster), caption: typeof image.caption === "string" ? image.caption : undefined }] : [];
  }) : [];
  const preview = gallery[0]?.video ? gallery[0].poster : gallery[0]?.url;
  const fallbackIcon = <Icon className="size-5" aria-hidden="true" />;
  useEffect(() => { if (copied) { const timer = setTimeout(() => setCopied(false), 2000); return () => clearTimeout(timer); } }, [copied]);
  return <Card className="flex flex-col gap-0 overflow-hidden p-0 shadow-none">
    {preview && <CatalogImage key={preview} src={preview} alt={gallery[0].caption ?? `${item.name} preview`} slot="marketplace-preview" className="bg-muted aspect-video w-full border-b object-contain" />}
    <div className="flex flex-1 flex-col p-5">
      <div className="mb-5 flex items-center justify-between gap-3">
        <span className="bg-muted text-foreground flex size-11 items-center justify-center overflow-hidden rounded-xl">{icon ? <CatalogImage key={icon} src={icon} alt="" slot="marketplace-icon" className="size-11 object-contain" fallback={fallbackIcon} /> : fallbackIcon}</span>
        <span className="text-muted-foreground text-xs">v{item.version}</span>
      </div>
      <h3 className="font-semibold tracking-tight">{item.name}</h3>
      <p className="text-muted-foreground mt-2 flex-1 text-sm leading-relaxed">{item.description}</p>
      <div className="mt-5 flex flex-wrap gap-1.5">{[...new Set(item.elements?.map((element) => element.kind) ?? ["app"])].map((kind) => <span key={kind} className="bg-muted text-muted-foreground rounded-md px-2 py-1 text-xs capitalize">{kind}</span>)}</div>
      <Button variant="outline" className="mt-5 w-full" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>{expanded ? "Hide details" : "View package"}</Button>
    </div>
    {expanded && <div className="bg-muted/30 border-t p-5 text-sm">
      {gallery.length > 0 && <div data-slot="marketplace-gallery" className="mb-5 space-y-4">{gallery.map((image, index) => <figure key={`${image.url}-${index}`}>
        {image.video ? <video src={image.url} poster={image.poster} controls preload="none" aria-label={image.caption ?? `${item.name} video ${index + 1}`} className="bg-muted aspect-video w-full rounded-md border object-contain" /> : <CatalogImage src={image.url} alt={image.caption ?? `${item.name} screenshot ${index + 1}`} slot="marketplace-gallery-image" className="bg-muted aspect-video w-full rounded-md border object-contain" />}
        {image.caption && <figcaption className="text-muted-foreground mt-2 text-xs">{image.caption}</figcaption>}
      </figure>)}</div>}
      {item.elements && <ul className="mb-4 space-y-3">{item.elements.map((element) => <li key={`${element.kind}/${element.name}`}><p className="font-medium">{element.name}</p>{element.description && <p className="text-muted-foreground mt-1 text-xs">{element.description}</p>}</li>)}</ul>}
      <a href={item.repo.replace(/\.git$/, "")} target="_blank" rel="noopener noreferrer" className="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 underline underline-offset-4">Source repository <ArrowUpRight className="size-3.5" aria-hidden="true" /></a>
      <p className="mt-4 mb-2 text-xs font-medium">Install from your terminal</p>
      <div className="bg-background flex items-start gap-2 rounded-md border p-2"><code className="min-w-0 flex-1 break-all text-xs leading-6">{command}</code><Button variant="ghost" size="icon" className="size-6 shrink-0" aria-label={`Copy install command for ${item.name}`} onClick={async () => { try { await navigator.clipboard.writeText(command); setCopied(true); setCopyError(false); } catch { setCopyError(true); } }}>{copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}</Button></div>
      <p role="status" className="text-muted-foreground mt-2 text-xs">{copyError ? "Select and copy the command above." : copied ? "Command copied." : "Installs on the machine where you run this command."}</p>
    </div>}
  </Card>;
}

export function Marketplace() {
  const [search, setSearch] = useState("");
  const [category, setCategory] = useState<Category>("All");
  const catalog = useQuery({ queryKey: ["marketplace-catalog"], queryFn: ({ signal }) => loadCatalog(signal), staleTime: 300000, retry: false });
  useEffect(() => { document.title = "Marketplace — ADI"; }, []);
  const items = (catalog.data ?? []).filter((item) => (!kinds[category] || item.elements?.some((element) => element.kind === kinds[category])) && `${item.name} ${item.description} ${item.elements?.map((element) => element.name).join(" ") ?? ""}`.toLowerCase().includes(search.trim().toLowerCase()));
  return <main aria-label="Marketplace" className="mx-auto w-full max-w-7xl py-5 sm:px-4 sm:py-8 lg:px-6 lg:py-10">
    <header className="mb-8"><p className="text-muted-foreground mb-3 flex items-center gap-2 text-xs font-medium"><Package className="size-4" aria-hidden="true" /> ADI COLLECTION</p><h2 className="text-3xl font-semibold tracking-tight">Make ADI your own.</h2><p className="text-muted-foreground mt-3 max-w-xl text-sm leading-relaxed">Discover agents, tools, and apps for your workspace. Pick a package, explore what’s inside, and install it on your machine.</p></header>
    <div className="mb-6 flex flex-wrap items-center justify-between gap-4">
      <div role="group" aria-label="Package category" className="flex flex-wrap gap-1">{categories.map((value) => <Button key={value} variant="ghost" size="sm" aria-pressed={category === value} className={cn(category === value && "bg-muted text-foreground")} onClick={() => setCategory(value)}>{value}</Button>)}</div>
      <div className="focus-within:ring-ring flex w-full items-center gap-2 rounded-lg border px-3 focus-within:ring-2 sm:w-64"><Search className="text-muted-foreground size-4" aria-hidden="true" /><input aria-label="Search marketplace" placeholder="Search marketplace…" value={search} onChange={(event) => setSearch(event.target.value)} className="placeholder:text-muted-foreground h-9 w-full min-w-0 bg-transparent text-sm outline-none" />{search && <button aria-label="Clear search" className="focus-visible:ring-ring rounded-sm focus-visible:ring-2" onClick={() => setSearch("")}><X className="text-muted-foreground size-4" /></button>}</div>
    </div>
    {catalog.isPending ? <p role="status" className="text-muted-foreground flex items-center gap-2 py-12 text-sm"><LoaderCircle className="size-4 motion-safe:animate-spin" /> Loading packages…</p> : catalog.isError ? <div role="alert" className="rounded-xl border p-6"><p>Couldn’t load the marketplace.</p><p className="text-muted-foreground mt-1 text-sm">Check your connection and try again.</p><Button variant="outline" className="mt-4" onClick={() => void catalog.refetch()}>Try again</Button></div> : <><p role="status" className="text-muted-foreground mb-4 text-xs">{items.length} {items.length === 1 ? "package" : "packages"}</p>{items.length ? <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">{items.map((item) => <ListingCard key={item.slug} item={item} />)}</div> : <div className="rounded-xl border border-dashed py-16 text-center"><Search className="text-muted-foreground mx-auto mb-4 size-6" /><h3 className="font-medium">No packages found</h3><p className="text-muted-foreground mt-2 text-sm">Try another search or category.</p><Button variant="outline" className="mt-5" onClick={() => { setSearch(""); setCategory("All"); }}>Reset filters</Button></div>}</>}
  </main>;
}
