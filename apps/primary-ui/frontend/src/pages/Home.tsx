import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowUpRight, Plus, Search, X } from "lucide-react";
import { Link } from "react-router-dom";

import { Button, Card, cn } from "@design-system/ui";

import { useChatSelection } from "../lib/app-chat-selection";
import { hasSource } from "../lib/app-chat-sources";
import { reportEvent } from "../lib/event-log";
import { getFleetMachines, getLocalApps, type HomeMachine } from "../lib/home-apps";

function appInitials(name: string) {
  const words = name.trim().split(/\s+/);
  return (words.length > 1 ? `${words[0][0]}${words[words.length - 1][0]}` : name.slice(0, 2)).toUpperCase();
}

export function Home() {
  const { newConversation, sources } = useChatSelection();
  const [query, setQuery] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);
  const local = useQuery({ queryKey: ["home-apps", "local"], queryFn: ({ signal }) => getLocalApps(signal), refetchInterval: 30000, retry: false });
  const fleet = useQuery({ queryKey: ["home-apps", "fleet"], queryFn: ({ signal }) => getFleetMachines(signal), refetchInterval: 30000, retry: false });
  const machines: HomeMachine[] = [{ id: "", label: "This machine", locked: false, apps: local.data ?? [] }, ...(fleet.data ?? [])];
  // The topbar machine selector decides whose apps are listed, exactly as it does for the chats.
  const remote = sources.nodes.length > 0;
  const shown = machines.filter((item) => hasSource(sources, item.id));
  const shownApps = shown.flatMap((item) => item.apps);
  const search = query.trim().toLowerCase();
  const apps = shownApps.filter((app) => `${app.name} ${app.description} ${app.source || "This machine"}`.toLowerCase().includes(search));
  const loading = (sources.local && local.isPending) || (remote && fleet.isPending);
  const localError = sources.local && local.isError ? local.error.message : null;
  const fleetError = remote && fleet.isError ? fleet.error.message : null;
  const issues = shown.filter((item) => item.issue);
  const issueKey = JSON.stringify(issues.map((item) => [item.id, item.issue]));

  // A chosen machine that is locked or unreachable is a background condition: it is recorded in
  // the event log once per change instead of taking space above the apps.
  useEffect(() => {
    for (const item of issues) reportEvent({ level: item.locked ? "info" : "error", source: item.id, area: "Apps", message: item.issue ?? "" });
  }, [issueKey]);

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
            <span data-slot="home-app-count" className="bg-muted text-muted-foreground rounded-md px-1.5 py-0.5 text-[11px] tabular-nums">{shownApps.length}</span>
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

        {(localError || fleetError) && <div className="mb-3 space-y-2 text-xs leading-5">
          {localError && <p role="alert" className="text-muted-foreground">This machine: {localError} <button type="button" className="focus-visible:ring-ring rounded underline underline-offset-4 outline-none focus-visible:ring-2" onClick={() => { void local.refetch(); }}>Retry</button></p>}
          {fleetError && <p role="alert" className="text-muted-foreground">Other machines: {fleetError} <button type="button" className="focus-visible:ring-ring rounded underline underline-offset-4 outline-none focus-visible:ring-2" onClick={() => { void fleet.refetch(); }}>Retry</button></p>}
        </div>}

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
          {loading ? "Loading apps…" : search ? apps.length ? `${apps.length} ${apps.length === 1 ? "app" : "apps"} found.` : `No apps found for “${query.trim()}”.` : apps.length ? `${apps.length} apps.` : localError || fleetError || issues.length ? "Some apps could not be loaded." : remote ? "No apps on the selected machines yet." : "No apps on this machine yet."}
        </p>
      </section>
    </main>
  );
}
