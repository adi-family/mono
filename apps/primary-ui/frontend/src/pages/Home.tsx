import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowUpRight, LoaderCircle, LockKeyhole, Plus, Search, X } from "lucide-react";
import { Link } from "react-router-dom";

import { Button, Card, cn } from "@design-system/ui";

import { useChatSelection } from "../lib/app-chat-selection";
import { getFleetMachines, getLocalApps, lockMachine, unlockMachine, type HomeMachine } from "../lib/home-apps";
import { UnlockMachineDialog } from "../components/UnlockMachineDialog";

function appInitials(name: string) {
  const words = name.trim().split(/\s+/);
  return (words.length > 1 ? `${words[0][0]}${words[words.length - 1][0]}` : name.slice(0, 2)).toUpperCase();
}

export function Home() {
  const { newConversation } = useChatSelection();
  const client = useQueryClient();
  const [query, setQuery] = useState("");
  const [machine, setMachine] = useState("*");
  const [unlocking, setUnlocking] = useState<string | null>(null);
  const [locking, setLocking] = useState<string | null>(null);
  const [lockError, setLockError] = useState<string | null>(null);
  const lockRequest = useRef<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const local = useQuery({ queryKey: ["home-apps", "local"], queryFn: ({ signal }) => getLocalApps(signal), refetchInterval: 30000, retry: false });
  const fleet = useQuery({ queryKey: ["home-apps", "fleet"], queryFn: ({ signal }) => getFleetMachines(signal), refetchInterval: 30000, retry: false });
  const machines: HomeMachine[] = [{ id: "", label: "This machine", locked: false, apps: local.data ?? [] }, ...(fleet.data ?? [])];
  const allApps = machines.flatMap((item) => item.apps);
  const selectedMachine = machine === "*" ? undefined : machines.find((item) => item.id === machine);
  const machineApps = selectedMachine?.apps ?? allApps;
  const search = query.trim().toLowerCase();
  const apps = machineApps.filter((app) => `${app.name} ${app.description} ${app.source || "This machine"}`.toLowerCase().includes(search));
  const loading = local.isPending || fleet.isPending;
  const issues = machines.filter((item) => item.issue);

  async function replaceFleet(next: HomeMachine[], signal?: AbortSignal) {
    // A catalog read started before the action must not restore an obsolete access state.
    await client.cancelQueries({ queryKey: ["home-apps", "fleet"] });
    if (signal?.aborted) return;
    client.setQueryData(["home-apps", "fleet"], next);
    for (const item of next.filter((item) => item.locked)) {
      for (const kind of ["agents", "conversations", "conversation"]) {
        client.removeQueries({ queryKey: ["app-chat", kind, item.id] });
      }
    }
    void client.invalidateQueries({ queryKey: ["app-chat"], refetchType: "none" });
  }

  async function unlock(username: string, password: string, signal: AbortSignal) {
    if (!unlocking) return;
    const node = unlocking;
    await replaceFleet(await unlockMachine(node, username, password, signal), signal);
  }

  function closeUnlock() {
    const node = unlocking;
    setUnlocking(null);
    // Query notifications can replace the original trigger just after the modal closes.
    requestAnimationFrame(() => {
      const trigger = document.querySelector<HTMLElement>(`button[aria-label="Unlock ${CSS.escape(node ?? "")}"]`);
      const filter = document.querySelector<HTMLElement>(`[data-slot="home-machine-filter"][data-machine="${CSS.escape(node ?? "")}"]`);
      (trigger ?? filter)?.focus({ preventScroll: true });
    });
  }

  async function lock(node: string) {
    if (lockRequest.current) return;
    lockRequest.current = node;
    setLocking(node);
    setLockError(null);
    try {
      await replaceFleet(await lockMachine(node));
      setMachine((current) => current === node ? "*" : current);
    } catch (error) {
      setLockError(`${node}: ${error instanceof Error ? error.message : "The machine could not be locked. Try again."}`);
    } finally { lockRequest.current = null; setLocking(null); }
  }

  function lockButton(node: string) {
    return <Button type="button" variant="ghost" size="icon" className="text-muted-foreground size-7 shrink-0" aria-label={`Lock ${node}`} title={`Lock ${node}`} disabled={locking !== null} disabledReason="A machine is being locked." onClick={() => { void lock(node); }}>
      {locking === node ? <LoaderCircle className="size-3 motion-safe:animate-spin" aria-hidden="true" /> : <LockKeyhole className="size-3" aria-hidden="true" />}
    </Button>;
  }

  useEffect(() => {
    if (machine !== "*" && !machines.some((item) => item.id === machine)) setMachine("*");
  }, [machine, fleet.data]); // A removed machine must not leave an invisible active filter.

  useEffect(() => {
    document.title = "Home — ADI";
  }, []);

  return (
    <main aria-label="Home" className="mx-auto w-full min-w-0 max-w-7xl flex-1 py-3 sm:px-4 sm:py-6 lg:px-6 lg:py-10">
      <header className="flex flex-wrap items-center justify-between gap-5">
        <div>
          <h2 className="text-2xl font-semibold tracking-tight sm:text-3xl">Your workspace</h2>
          <p className="text-muted-foreground mt-2 text-sm">Open an app or start a new chat.</p>
        </div>
        <Button data-slot="home-new-chat" aria-label="New chat" size="lg" className="gap-2" onClick={() => newConversation()}>
          <Plus className="size-4" aria-hidden="true" />
          New chat
        </Button>
      </header>

      <section aria-labelledby="home-apps-title" className="mt-10 sm:mt-12">
        <div className="mb-5 flex flex-wrap items-center justify-between gap-4">
          <div className="flex items-center gap-2.5">
            <h3 id="home-apps-title" className="text-sm font-semibold">All apps</h3>
            <span data-slot="home-app-count" className="bg-muted text-muted-foreground rounded-md px-1.5 py-0.5 text-[11px] tabular-nums">{allApps.length}</span>
          </div>
          <div className="relative w-full sm:w-60">
            <Search className="text-muted-foreground pointer-events-none absolute top-2.5 left-3 size-4" aria-hidden="true" />
            <input
              ref={searchRef}
              type="search"
              aria-label="Search apps"
              placeholder="Search apps…"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              className="border-input placeholder:text-muted-foreground focus-visible:ring-ring h-9 w-full rounded-lg border bg-transparent pr-9 pl-9 text-sm outline-none focus-visible:ring-2 [&::-webkit-search-cancel-button]:appearance-none"
            />
            {query && <Button variant="ghost" size="icon" className="absolute top-1 right-1 size-7" aria-label="Clear app search" onClick={() => { setQuery(""); searchRef.current?.focus(); }}>
              <X className="size-3.5" aria-hidden="true" />
            </Button>}
          </div>
        </div>

        <nav aria-label="Filter apps by machine" className="mb-5 flex flex-wrap gap-2">
          {[{ id: "*", label: "All machines", apps: allApps }, ...machines.filter((item) => !item.issue)].map((item) => (
            <div key={item.id} className="flex min-w-0 items-center gap-0.5">
              <button type="button" aria-pressed={machine === item.id} data-slot="home-machine-filter" data-machine={item.id} className={cn("focus-visible:ring-ring rounded-full px-3 py-1.5 text-xs outline-none transition-colors focus-visible:ring-2", machine === item.id ? "bg-foreground text-background" : "text-muted-foreground hover:bg-muted hover:text-foreground")} onClick={() => setMachine(item.id)}>
                {item.label}<span className="ml-1.5 opacity-60 tabular-nums">{item.apps.length}</span>
              </button>
              {item.id && item.id !== "*" && lockButton(item.id)}
            </div>
          ))}
        </nav>

        <div className="mb-3 space-y-2 text-xs leading-5">
          {local.isError && <p role="alert" className="text-muted-foreground">This machine: {local.error.message} <button type="button" className="focus-visible:ring-ring rounded underline underline-offset-4 outline-none focus-visible:ring-2" onClick={() => { void local.refetch(); }}>Retry</button></p>}
          {fleet.isError && <p role="alert" className="text-muted-foreground">Other machines: {fleet.error.message} <button type="button" className="focus-visible:ring-ring rounded underline underline-offset-4 outline-none focus-visible:ring-2" onClick={() => { void fleet.refetch(); }}>Retry</button></p>}
          {lockError && <p role="alert" className="text-destructive">{lockError}</p>}
          {issues.length > 0 && <ul aria-label="Unavailable machines" className="text-muted-foreground flex flex-wrap gap-x-5 gap-y-2">
            {issues.map((item) => <li key={item.id} className="flex min-w-0 items-center gap-1.5">
              <LockKeyhole className="size-3 shrink-0" aria-hidden="true" />
              <a href={item.href} className="focus-visible:ring-ring rounded underline underline-offset-4 outline-none focus-visible:ring-2 [overflow-wrap:anywhere]" title={item.issue}>{item.label}</a>
              <span>{item.locked ? "Locked" : "Unavailable"}</span>
              {item.locked ? <Button type="button" variant="outline" size="sm" className="ml-1 h-6 px-2 text-xs" aria-label={`Unlock ${item.id}`} disabled={locking !== null} disabledReason="A machine is being locked." onClick={() => setUnlocking(item.id)}>Unlock</Button> : lockButton(item.id)}
            </li>)}
          </ul>}
        </div>

        <ul aria-label="Applications" aria-busy={loading} className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,8rem),1fr))] gap-2 sm:gap-3">
          {apps.map(({ key, name, description, source, href, notice, icon: Icon }) => {
            const label = `${name} · ${source || "This machine"}${notice ? ` · ${notice}` : ""}`;
            const content = <Card className="group-hover:bg-muted/50 group-focus-visible:bg-muted/50 h-full items-center gap-3 rounded-xl border-transparent bg-transparent px-2 py-4 text-center shadow-none transition-colors">
              <span className="bg-muted relative flex size-14 shrink-0 items-center justify-center rounded-2xl">
                {Icon ? <Icon className="size-7" aria-hidden="true" /> : <span className="text-lg font-semibold tracking-tight" aria-hidden="true">{appInitials(name)}</span>}
                {href && <ArrowUpRight className="text-muted-foreground absolute -top-1 -right-1 size-3.5 opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100" aria-hidden="true" />}
              </span>
              <div className="w-full min-w-0">
                <h4 className="text-sm leading-5 font-medium [overflow-wrap:anywhere]">{name}</h4>
                <p className="text-muted-foreground mt-1 text-[11px] leading-4 [overflow-wrap:anywhere]">{source || "This machine"}</p>
                {notice && <p className="text-muted-foreground mt-1 text-[11px] leading-4">{notice}</p>}
              </div>
            </Card>;
            const className = "group focus-visible:ring-ring block h-full rounded-xl outline-none focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:ring-offset-background";
            const props = { "aria-label": label, title: [name, description, source || "This machine", notice].filter(Boolean).join("\n"), className };
            return <li key={key} data-source={source} className="min-w-0">
              {!href ? <div {...props} data-slot="home-app-unavailable">{content}</div>
                : href.startsWith("/") ? <Link {...props} data-slot="home-app-link" to={href}>{content}</Link>
                  : <a {...props} data-slot="home-app-link" href={href}>{content}</a>}
            </li>;
          })}
        </ul>
        <p role="status" className={cn("text-muted-foreground mt-4 text-sm", !loading && !search && apps.length > 0 && "sr-only")}>
          {loading ? "Loading apps across your machines…" : search ? apps.length ? `${apps.length} ${apps.length === 1 ? "app" : "apps"} found.` : `No apps found for “${query.trim()}”.` : apps.length ? `${apps.length} apps.` : local.isError || fleet.isError ? "Some apps could not be loaded." : "No apps on this machine yet."}
        </p>
      </section>
      {unlocking && <UnlockMachineDialog key={unlocking} machine={unlocking} onUnlock={unlock} onClose={closeUnlock} />}
    </main>
  );
}
