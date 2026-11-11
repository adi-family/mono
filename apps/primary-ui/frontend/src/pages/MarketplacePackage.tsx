import { useEffect, useRef, useState, type ReactNode } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, ArrowUpRight, LoaderCircle, Package } from "lucide-react";
import { Button, Card } from "@design-system/ui";
import { ChatMarkdown } from "../components/ChatMarkdown";
import { CatalogImage, imageUrl } from "./Marketplace";
import { sourceName } from "../lib/app-chat-sources";
import { elementSpec, hasDashboardElements, installHref, loadDashboards, loadMarketplace, marketplaceDashboardsKey, marketplaceKey, marketplaceProjectsKey, marketplaceRequest, type MarketplaceApp, type MarketplaceElement, type MarketplaceState } from "../lib/marketplace";
import { useMarketplaceSource } from "../lib/marketplace-source";

const inputClass = "border-input bg-background focus-visible:ring-ring h-10 w-full rounded-md border px-3 text-sm outline-none focus-visible:ring-2";

function Dialog({ title, onClose, busy, children }: { title: string; onClose: () => void; busy: boolean; children: ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    ref.current?.showModal();
    return () => { ref.current?.close(); if (previous?.isConnected) previous.focus({ preventScroll: true }); };
  }, []);
  return <dialog ref={ref} aria-label={title} onCancel={(event) => { event.preventDefault(); if (!busy) onClose(); }} className="bg-popover text-popover-foreground fixed top-1/2 left-1/2 m-0 max-h-[90dvh] w-[calc(100vw_-_2rem)] max-w-lg -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-xl border p-6 shadow-xl backdrop:bg-black/50">
    <h2 className="mb-5 text-lg font-semibold">{title}</h2>{children}
  </dialog>;
}

function PackagePage({ app, source }: { app: MarketplaceApp; source?: string }) {
  const cache = useQueryClient();
  const key = marketplaceKey(source);
  const machine = source ?? "this machine";
  const [search, setSearch] = useSearchParams();
  const [install, setInstall] = useState<{ element?: string } | null>(search.get("install") === "1" ? {} : null);
  const [destination, setDestination] = useState("new");
  const [projectName, setProjectName] = useState(app.elements.find((element) => element.kind === "project")?.name ?? app.name);
  const [project, setProject] = useState("");
  const [name, setName] = useState(app.name);
  const [start, setStart] = useState(true);
  const [mediaIndex, setMediaIndex] = useState(0);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [confirmation, setConfirmation] = useState<{ title: string; description: string; path: string; body: unknown } | null>(null);
  const projects = useQuery({ queryKey: marketplaceProjectsKey(source), queryFn: ({ signal }) => marketplaceRequest<{ projects: { id: string; name: string; archived_at?: number | null }[] }>(source, "/projects", undefined, signal), enabled: install !== null });
  // A bundle's dashboard element is a row in the machine's Dashboards registry: archived until started, then on its host.
  const dashboards = useQuery({ queryKey: marketplaceDashboardsKey(source), queryFn: loadDashboards(source), enabled: hasDashboardElements(app), staleTime: 30000, refetchInterval: 30000 });
  const dashboardOf = (element: MarketplaceElement) => element.kind === "dashboards" && element.id ? dashboards.data?.find((row) => row.id === element.id) : undefined;
  const legacy = !app.bundle && app.elements.length === 0;
  const address = { marketplace: app.marketplace, slug: app.slug };
  const elements = app.bundle?.elements.length ? app.bundle.elements : app.elements;
  const icon = imageUrl(app.icon);
  const selectedIndex = Math.min(mediaIndex, Math.max(0, app.gallery.length - 1));
  const selectedMedia = app.gallery[selectedIndex];
  const installed = app.installs.length > 0 || !!app.bundle?.installs.length;

  useEffect(() => { document.title = `${app.name} — Marketplace — ADI`; }, [app.name]);

  function closeInstall() {
    setInstall(null);
    if (search.has("install")) { const next = new URLSearchParams(search); next.delete("install"); setSearch(next, { replace: true }); }
  }
  function openInstall(element?: string) {
    setError(null); setDestination("new"); setProject(""); setProjectName(app.elements.find((item) => item.kind === "project")?.name ?? app.name); setName(app.name); setStart(true); setInstall({ element });
  }
  async function act(path: string, body: unknown): Promise<boolean> {
    if (pending.current) return false;
    pending.current = true; setBusy(true); setError(null); setMessage(null);
    try {
      const result = await marketplaceRequest<{ state: MarketplaceState; message: string }>(source, `/marketplace/${path}`, body);
      await cache.cancelQueries({ queryKey: key });
      cache.setQueryData(key, result.state);
      setMessage(result.message);
      void cache.invalidateQueries({ queryKey: marketplaceProjectsKey(source) });
      void cache.invalidateQueries({ queryKey: marketplaceDashboardsKey(source) });
      return true;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The request failed. Try again.");
      return false;
    } finally { pending.current = false; setBusy(false); }
  }
  const disabled = { disabled: busy, disabledReason: "Wait for the current marketplace action to finish." };

  return <main data-slot="marketplace-package" aria-label={app.name} className="mx-auto w-full max-w-5xl space-y-8 py-5 sm:px-4 sm:py-8 lg:px-6">
    <Link to="/marketplace" className="text-muted-foreground hover:text-foreground inline-flex items-center gap-2 text-sm"><ArrowLeft className="size-4" />All packages</Link>
    <header className="flex flex-wrap items-start gap-5">
      <div className="bg-muted flex size-16 shrink-0 items-center justify-center overflow-hidden rounded-xl">{icon ? <CatalogImage src={icon} alt="" slot="marketplace-package-icon" className="size-16 object-contain" fallback={<Package className="size-7" />} /> : <Package className="size-7" />}</div>
      <div className="min-w-0 flex-1"><p className="text-muted-foreground mb-2 text-xs">From {app.marketplace} · <span data-slot="marketplace-package-machine">On {sourceName(source)}</span></p><h2 className="text-3xl font-semibold tracking-tight">{app.name} {app.version && <span className="text-muted-foreground text-sm font-normal">v{app.version}</span>}</h2><p className="text-muted-foreground mt-3 text-sm leading-relaxed">{app.description}</p><div className="mt-3 flex flex-wrap gap-2">{app.keywords?.map((word) => <span key={word} className="bg-muted text-muted-foreground rounded-md px-2 py-1 text-xs">{word}</span>)}</div></div>
      <Button {...disabled} className="w-full sm:w-auto" data-slot="marketplace-install" onClick={() => openInstall()}>{installed ? "Install another copy" : elements.length > 1 ? "Install everything" : "Install"}</Button>
    </header>
    {message && <p role="status" className="bg-muted rounded-lg border p-4 text-sm whitespace-pre-wrap">{message}</p>}
    {error && !install && !confirmation && <p role="alert" className="text-destructive rounded-lg border p-4 text-sm">{error}</p>}
    {selectedMedia && <section aria-label="Gallery" data-slot="marketplace-package-gallery" className="space-y-4"><figure>
      {selectedMedia.kind === "video" && /^(https:\/\/|data:video\/)/i.test(selectedMedia.url) ? <video key={selectedMedia.url} src={selectedMedia.url} poster={imageUrl(selectedMedia.poster)} controls preload="none" aria-label={selectedMedia.caption ?? `${app.name} video ${selectedIndex + 1}`} className="bg-muted aspect-video w-full rounded-xl border object-contain" /> : imageUrl(selectedMedia.url) && <CatalogImage key={selectedMedia.url} src={selectedMedia.url} alt={selectedMedia.caption ?? `${app.name} screenshot ${selectedIndex + 1}`} slot="marketplace-package-image" className="bg-muted aspect-video w-full rounded-xl border object-contain" />}
      <div className="mt-3 flex flex-wrap items-center justify-between gap-3"><figcaption className="text-muted-foreground text-sm">{selectedMedia.caption ?? `Preview ${selectedIndex + 1}`}</figcaption>{app.gallery.length > 1 && <div className="flex items-center gap-2"><Button variant="outline" size="sm" aria-label="Previous preview" onClick={() => setMediaIndex((selectedIndex + app.gallery.length - 1) % app.gallery.length)}>Previous</Button><span className="text-muted-foreground text-xs">{selectedIndex + 1} / {app.gallery.length}</span><Button variant="outline" size="sm" aria-label="Next preview" onClick={() => setMediaIndex((selectedIndex + 1) % app.gallery.length)}>Next</Button></div>}</div>
    </figure></section>}
    {elements.length > 0 && <section aria-label="What's included"><h3 className="mb-4 text-lg font-semibold">What’s included</h3><Card className="gap-0 p-0 shadow-none"><ul className="divide-y">{elements.map((element) => <li key={elementSpec(element)} className="flex flex-wrap items-center gap-3 p-4"><div className="min-w-0 flex-1"><p className="text-sm font-medium">{element.name}</p><p className="text-muted-foreground mt-1 text-xs">{element.kind}</p>{element.description && <p className="text-muted-foreground mt-2 text-sm">{element.description}</p>}</div><Button {...disabled} variant="outline" size="sm" aria-label={`Install ${element.name}`} onClick={() => openInstall(elementSpec(element))}>Install</Button></li>)}</ul></Card></section>}
    {installed && <section aria-label={source ? `Installed on ${source}` : "Installed here"} data-slot="marketplace-installed" className="space-y-4"><h3 className="text-lg font-semibold">{source ? `Installed on ${source}` : "Installed here"}</h3>
      {app.installs.map((copy) => { const href = installHref(copy.host, source); return <Card key={copy.id} className="gap-4 p-5 shadow-none"><div className="flex flex-wrap items-center justify-between gap-3"><div><h4 className="text-sm font-semibold">{copy.name}</h4><p className="text-muted-foreground mt-1 text-xs">{copy.id} · {copy.started ? "Started" : "Not started"} · {copy.commit.slice(0, 9)}</p></div><div className="flex flex-wrap gap-2">
        {copy.started && href ? <Button asChild variant="outline" size="sm"><a href={href} target="_blank" rel="noopener noreferrer">Open <ArrowUpRight className="size-3.5" /></a></Button> : !copy.started && <Button {...disabled} variant="outline" size="sm" onClick={() => void act("start", { id: copy.id })}>Start</Button>}
        {copy.outdated && <><Button {...disabled} variant="outline" size="sm" onClick={() => void act("update", { id: copy.id, force: false })}>Update</Button><Button {...disabled} variant="ghost" size="sm" onClick={() => { setError(null); setConfirmation({ title: "Force update", description: `Reset ${copy.name} to the published commit? Local changes and commits will be lost.`, path: "update", body: { id: copy.id, force: true } }); }}>Force update…</Button></>}
      </div></div></Card>; })}
      {app.bundle?.installs.map((copy) => <Card key={copy.project ?? "global"} className="gap-4 p-5 shadow-none"><div className="flex flex-wrap items-center justify-between gap-3"><div><h4 className="text-sm font-semibold">{copy.project_name ?? copy.project ?? "Globally"}</h4><p className="text-muted-foreground mt-1 text-xs">{copy.elements.length} elements · {copy.commit.slice(0, 9)}</p></div>{copy.outdated && <Button {...disabled} variant="outline" size="sm" onClick={() => void act("bundle/update", { ...address, project: copy.project ?? null, force: [] })}>Update</Button>}</div>
        {copy.missing_secrets.length > 0 && <p className="text-destructive text-sm">Missing secrets: {copy.missing_secrets.join(", ")}</p>}
        <ul className="divide-y">{copy.elements.map((element) => { const dashboard = dashboardOf(element); const dashboardHref = dashboard && dashboard.archived_at == null ? installHref(dashboard.host, source) : null; const started = dashboard ? dashboard.archived_at == null : element.started; return <li key={elementSpec(element)} className="flex flex-wrap items-center justify-between gap-3 py-3"><div><p className="text-sm font-medium">{element.name}</p><p className="text-muted-foreground mt-1 text-xs">{element.id ?? elementSpec(element)}{started === null || started === undefined ? "" : started ? " · Started" : " · Not started"}</p></div><div className="flex flex-wrap gap-2">
          {element.kind === "services" && !element.started && <Button {...disabled} variant="outline" size="sm" onClick={() => void act("start-service", { ...address, name: element.name, project: copy.project ?? null })}>Start</Button>}
          {dashboard && element.id && (dashboard.archived_at == null
            ? dashboardHref && <Button asChild variant="outline" size="sm"><a href={dashboardHref} target="_blank" rel="noopener noreferrer" aria-label={`Open ${element.name}`}>Open <ArrowUpRight className="size-3.5" /></a></Button>
            : <Button {...disabled} variant="outline" size="sm" aria-label={`Start ${element.name}`} onClick={() => void act("start", { id: element.id })}>Start</Button>)}
          {copy.outdated && <Button {...disabled} variant="ghost" size="sm" aria-label={`Force update ${element.name}`} onClick={() => { setError(null); setConfirmation({ title: "Force update", description: `Overwrite local changes to ${elementSpec(element)} in ${copy.project_name ?? copy.project ?? "global scope"}?`, path: "bundle/update", body: { ...address, project: copy.project ?? null, force: [elementSpec(element)] } }); }}>Force update…</Button>}
          <Button {...disabled} variant="ghost" size="sm" aria-label={`Uninstall ${element.name}`} onClick={() => { setError(null); setConfirmation({ title: "Uninstall element", description: `Uninstall ${elementSpec(element)} from ${copy.project_name ?? copy.project ?? "global scope"}? Other elements and copies remain installed.`, path: "uninstall", body: { ...address, project: copy.project ?? null, element: elementSpec(element) } }); }}>Uninstall…</Button>
        </div></li>; })}</ul>
      </Card>)}
    </section>}
    {app.readme && <section aria-label="About this package"><h3 className="mb-4 text-lg font-semibold">About this package</h3><ChatMarkdown>{app.readme}</ChatMarkdown></section>}
    <section aria-label="Repository" className="space-y-2 border-t pt-6 text-sm"><h3 className="font-semibold">Repository</h3><a href={app.repo.replace(/\.git$/, "")} target="_blank" rel="noopener noreferrer" className="text-muted-foreground inline-flex items-center gap-1 break-all underline underline-offset-4">{app.repo.replace(/\.git$/, "")} <ArrowUpRight className="size-3.5 shrink-0" /></a><p className="text-muted-foreground break-all font-mono text-xs">Commit: {app.commit}</p>{app.branch && <p className="text-muted-foreground text-xs">Branch: {app.branch}</p>}</section>
    {install && <Dialog title={`Install ${app.name} on ${machine}`} busy={busy} onClose={closeInstall}><form data-slot="marketplace-install-form" className="space-y-5" aria-busy={busy} onSubmit={async (event) => { event.preventDefault(); if (await act("install", { ...address, element: install.element ?? null, name: legacy ? name.trim() : "", start, project: destination === "existing" ? project : null, new_project: destination === "new" ? projectName.trim() : null })) closeInstall(); }}>
      {install.element && <p className="text-muted-foreground text-sm">Installing {install.element} on its own.</p>}
      <fieldset disabled={busy} className="space-y-4"><legend className="mb-3 text-sm font-medium">Where should this be installed?</legend>
        {[["new", "A new project"], ["existing", "An existing project"], ["global", "Globally"]].map(([value, label]) => <label key={value} className="flex items-center gap-2 text-sm"><input type="radio" name="destination" value={value} checked={destination === value} onChange={() => setDestination(value)} />{label}</label>)}
        {destination === "new" && <label className="block space-y-2 text-sm"><span>Project name</span><input aria-label="Project name" required value={projectName} onChange={(event) => setProjectName(event.target.value)} className={inputClass} /></label>}
        {destination === "existing" && <><label className="block space-y-2 text-sm"><span>Project</span><select aria-label="Project" required value={project} onChange={(event) => setProject(event.target.value)} className={inputClass}><option value="">{projects.isPending ? "Loading projects…" : "Choose a project"}</option>{projects.data?.projects.filter((item) => !item.archived_at).map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label>{projects.isError && <p role="alert" className="text-destructive text-sm">Couldn’t load projects. <button type="button" className="underline" onClick={() => void projects.refetch()}>Try again</button></p>}</>}
        {destination === "global" && <p className="text-muted-foreground text-sm">Available globally on {machine}. Use a project to keep multiple copies separate.</p>}
        {legacy && <><label className="block space-y-2 text-sm"><span>Copy name</span><input aria-label="Copy name" required value={name} onChange={(event) => setName(event.target.value)} className={inputClass} /></label><label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={start} onChange={(event) => setStart(event.target.checked)} />Start it right away</label></>}
      </fieldset>
      {error && <p role="alert" className="text-destructive text-sm">{error}</p>}
      <div className="flex justify-end gap-2"><Button type="button" variant="outline" {...disabled} onClick={closeInstall}>Cancel</Button><Button type="submit" disabled={busy || (destination === "new" && !projectName.trim()) || (destination === "existing" && !project) || (legacy && !name.trim())} disabledReason={busy ? "Installation is in progress." : "Choose a destination and fill in the required names."}>{busy && <LoaderCircle className="size-4 animate-spin" />}{busy ? "Installing…" : "Install"}</Button></div>
    </form></Dialog>}
    {confirmation && <Dialog title={confirmation.title} busy={busy} onClose={() => setConfirmation(null)}><p className="text-muted-foreground text-sm">{confirmation.description}</p>{error && <p role="alert" className="text-destructive mt-4 text-sm">{error}</p>}<div className="mt-6 flex justify-end gap-2"><Button variant="outline" {...disabled} onClick={() => setConfirmation(null)}>Cancel</Button><Button variant="destructive" {...disabled} onClick={async () => { if (await act(confirmation.path, confirmation.body)) setConfirmation(null); }}>{busy ? "Working…" : confirmation.title}</Button></div></Dialog>}
  </main>;
}

export function MarketplacePackage() {
  const { marketplace, slug } = useParams();
  const [source] = useMarketplaceSource();
  const catalog = useQuery({ queryKey: marketplaceKey(source), queryFn: loadMarketplace(source), staleTime: 30000, refetchInterval: 30000 });
  const app = catalog.data?.apps.find((item) => item.marketplace === marketplace && item.slug === slug);
  if (catalog.isPending) return <p role="status" className="p-6 text-sm">Loading package…</p>;
  if (catalog.isError) return <div role="alert" className="space-y-4 p-6"><p>Couldn’t load this package from {source ?? "this machine"}.</p><p className="text-muted-foreground text-sm">{catalog.error.message}</p><Button variant="outline" onClick={() => void catalog.refetch()}>Try again</Button></div>;
  if (!app) return <main className="space-y-4 p-6"><h2 className="text-xl font-semibold">Package not found</h2><p className="text-muted-foreground text-sm">{sourceName(source)} doesn’t have {marketplace}/{slug} in its marketplace catalog.</p><Link to="/marketplace" className="underline">All packages</Link></main>;
  return <PackagePage key={`${source ?? ""}/${marketplace}/${slug}`} app={app} source={source} />;
}
