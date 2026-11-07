import { useEffect, useId, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, Filter, LockKeyhole, Network, Plus, Search, Star } from "lucide-react";

import { Button, cn } from "@design-system/ui";

import { useAppChat } from "../lib/app-chat-state";
import { appHref } from "../lib/app-api";
import { conversationAge, conversationFilters, conversationGroups, conversationSections } from "../lib/app-chat-sidebar";
import { sameConversation } from "../lib/app-chat-sources";
import { conversationHref } from "../lib/app-chat-transcript";

import { ChatContextMenu, type ChatContextTarget } from "./ChatContextMenu";

type Menu = "filters" | "sources";
const menuItemClass = "focus:bg-accent hover:bg-accent flex min-h-8 w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs outline-none disabled:opacity-50 disabled:hover:bg-transparent";

function SidebarMenu({ menu, anchor, onClose }: { menu: Menu; anchor: RefObject<HTMLButtonElement | null>; onClose: (restoreFocus?: boolean) => void }) {
  const { sources, fleetNodes, fleetLoading, fleetError, toggleSource, filter, setFilter, group, setGroup } = useAppChat();
  const element = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: 0, top: 0 });

  useLayoutEffect(() => {
    const bounds = anchor.current?.getBoundingClientRect();
    if (bounds) setPosition({ left: Math.max(8, Math.min(bounds.left, window.innerWidth - 248)), top: bounds.bottom + 5 });
    const selected = element.current?.querySelector<HTMLButtonElement>('[aria-checked="true"]:not(:disabled)')
      ?? element.current?.querySelector<HTMLButtonElement>('[role^="menuitem"]:not(:disabled)');
    selected?.focus();
  }, [anchor, menu]);

  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !element.current?.contains(event.target) && !anchor.current?.contains(event.target)) onClose(false);
    };
    const reposition = () => onClose(false);
    document.addEventListener("pointerdown", outside);
    window.addEventListener("resize", reposition);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("resize", reposition);
    };
  }, [anchor, onClose]);

  return createPortal(
    <div
      ref={element}
      data-slot="app-chat-menu"
      role="menu"
      aria-label={menu === "filters" ? "Conversation filters" : "Conversation sources"}
      className="bg-popover text-popover-foreground border-border fixed z-50 max-h-[70svh] w-60 overflow-y-auto rounded-md border p-1 shadow-md"
      style={position}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          onClose();
        } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
          event.preventDefault();
          const items = [...(element.current?.querySelectorAll<HTMLElement>('[role^="menuitem"]:not(:disabled)') ?? [])];
          const current = items.indexOf(document.activeElement as HTMLElement);
          const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
          items[next]?.focus();
        } else if (event.key === "Tab") onClose();
      }}
    >
      {menu === "filters" ? (
        <>
          <p className="text-muted-foreground px-2 py-1.5 text-[11px] font-medium">Show sessions</p>
          {conversationFilters.map((option) => (
            <button key={option.value} type="button" role="menuitemradio" aria-checked={filter === option.value} className={menuItemClass} onClick={() => { setFilter(option.value); onClose(); }}>
              <Check aria-hidden="true" className={cn("size-3.5 shrink-0", filter !== option.value && "invisible")} />
              {option.label}
            </button>
          ))}
          <div role="separator" className="bg-border my-1 h-px" />
          <p className="text-muted-foreground px-2 py-1.5 text-[11px] font-medium">Group by</p>
          {conversationGroups.map((option) => (
            <button key={option.value} type="button" role="menuitemradio" aria-checked={group === option.value} className={menuItemClass} onClick={() => { setGroup(option.value); onClose(); }}>
              <Check aria-hidden="true" className={cn("size-3.5 shrink-0", group !== option.value && "invisible")} />
              {option.label}
            </button>
          ))}
        </>
      ) : (
        <>
          <p className="text-muted-foreground px-2 py-1.5 text-[11px] font-medium">Sessions from</p>
          <button type="button" role="menuitemcheckbox" aria-checked={sources.local} title={sources.local && sources.nodes.length === 0 ? "Keep at least one source selected." : undefined} className={menuItemClass} onClick={() => toggleSource()}>
            <Check aria-hidden="true" className={cn("size-3.5 shrink-0", !sources.local && "invisible")} />
            This machine
          </button>
          {fleetNodes.map((node) => {
            const selected = sources.nodes.includes(node.node);
            return (
              <button key={node.node} type="button" role="menuitemcheckbox" aria-checked={selected} disabled={node.locked && !selected} title={node.locked && !selected ? "Unlock this source on the Fleet page first." : undefined} className={menuItemClass} onClick={() => toggleSource(node.node)}>
                <Check aria-hidden="true" className={cn("size-3.5 shrink-0", !selected && "invisible")} />
                <span className="min-w-0 flex-1 truncate">{node.node}</span>
                {node.locked && <LockKeyhole aria-hidden="true" className="text-muted-foreground size-3 shrink-0" />}
              </button>
            );
          })}
          {/* Stored sources remain removable if the fleet is offline or they were unpaired. */}
          {sources.nodes.filter((source) => !fleetNodes.some((node) => node.node === source)).map((source) => (
            <button key={source} type="button" role="menuitemcheckbox" aria-checked="true" className={menuItemClass} onClick={() => toggleSource(source)}>
              <Check aria-hidden="true" className="size-3.5 shrink-0" />
              <span className="min-w-0 truncate">{source}</span>
            </button>
          ))}
          {fleetLoading && <p role="status" className="text-muted-foreground px-2 py-2 text-xs">Loading sources…</p>}
          {fleetError && <p role="alert" className="text-muted-foreground px-2 py-2 text-xs leading-relaxed">{fleetError}</p>}
          {!fleetLoading && !fleetError && fleetNodes.length === 0 && <p className="text-muted-foreground px-2 py-2 text-xs leading-relaxed">No paired nodes yet.</p>}
          <div role="separator" className="bg-border my-1 h-px" />
          <a role="menuitem" href={appHref("/extended/settings/fleet")} className={menuItemClass}>Manage sources in Fleet</a>
        </>
      )}
    </div>,
    document.body,
  );
}

export function AppChatSidebar() {
  const { conversations, active, isLoading, indexError, sourceStates, sources, filter, group, loadMore, openConversation, newConversation, refresh } = useAppChat();
  const [contextTarget, setContextTarget] = useState<ChatContextTarget | null>(null);
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [visibleCounts, setVisibleCounts] = useState<Record<string, number>>({});
  const [expanded, setExpanded] = useState(false);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);
  const toggleRef = useRef<HTMLButtonElement>(null);
  const filterRef = useRef<HTMLButtonElement>(null);
  const sourceRef = useRef<HTMLButtonElement>(null);
  const navigationId = useId();
  const sections = conversationSections(conversations, sourceStates, group, query);
  const count = sections.reduce((total, section) => total + section.conversations.length, 0);
  const selectedTitle = conversations.find((conversation) => sameConversation(conversation, active))?.title;
  const sourceCount = Number(sources.local) + sources.nodes.length;
  const currentFilter = conversationFilters.find((option) => option.value === filter)?.label;
  const closeMenu = (restoreFocus = true) => {
    if (restoreFocus) (menu === "filters" ? filterRef : sourceRef).current?.focus();
    setMenu(null);
  };

  return (
    <aside data-slot="app-chat-sidebar" className="flex min-h-0 min-w-0 flex-col md:h-full" onKeyDown={(event) => {
      if (event.key === "Escape" && expanded) {
        event.preventDefault();
        setExpanded(false);
        toggleRef.current?.focus();
      }
    }}>
      <button ref={toggleRef} type="button" aria-expanded={expanded} aria-controls={navigationId} className="focus-visible:ring-ring flex w-full items-center gap-3 rounded-md px-1 py-1 text-left text-sm outline-none focus-visible:ring-2 md:hidden" onClick={() => setExpanded((value) => !value)}>
        <span className="font-medium">Conversations</span>
        <span className="text-muted-foreground min-w-0 flex-1 truncate text-xs">{selectedTitle ?? "New chat"}</span>
        <ChevronDown aria-hidden="true" className={cn("size-4 shrink-0", expanded && "rotate-180")} />
      </button>

      <div id={navigationId} className={cn("mt-3 min-h-0 flex-1 flex-col md:mt-0 md:flex", expanded ? "flex" : "hidden")}>
        <div className="mb-2 flex shrink-0 items-center gap-1 px-1">
          <h2 className="mr-auto pl-1 text-sm font-semibold">Chats</h2>
          <Button ref={sourceRef} variant="ghost" size="icon" className={cn("size-7 shrink-0", sourceCount > 1 && "w-auto min-w-7 gap-1 px-1.5", sourceCount > 1 || !sources.local ? "text-foreground" : "text-muted-foreground")} aria-label="Conversation sources" title={`Sessions from ${[...(sources.local ? ["This machine"] : []), ...sources.nodes].join(", ")}`} aria-haspopup="menu" aria-expanded={menu === "sources"} onClick={() => setMenu((current) => current === "sources" ? null : "sources")}>
            <Network aria-hidden="true" className="size-3.5" />
            {sourceCount > 1 && <span className="text-[10px]">{sourceCount}</span>}
          </Button>
          <Button ref={filterRef} variant="ghost" size="icon" className={cn("size-7 shrink-0", filter === "all" ? "text-muted-foreground" : "text-foreground bg-foreground/8")} aria-label="Filter conversations" title={currentFilter} aria-haspopup="menu" aria-expanded={menu === "filters"} onClick={() => setMenu((current) => current === "filters" ? null : "filters")}>
            <Filter aria-hidden="true" className="size-3.5" />
          </Button>
          <Button variant="ghost" size="icon" className="text-muted-foreground size-7 shrink-0 hover:text-foreground" aria-label="New conversation" title="New conversation" onClick={() => { newConversation(); setExpanded(false); }}>
            <Plus aria-hidden="true" className="size-4" />
          </Button>
        </div>

        <div className="relative mb-2 shrink-0">
          <Search aria-hidden="true" className="text-muted-foreground pointer-events-none absolute top-2.5 left-2.5 size-3.5" />
          <input type="search" aria-label="Search conversations" placeholder="Search chats" value={query} onChange={(event) => setQuery(event.target.value)} className="border-input/60 bg-background/40 placeholder:text-muted-foreground focus-visible:ring-ring h-8 w-full rounded-md border py-1 pr-2 pl-8 text-xs outline-none focus-visible:ring-2" />
        </div>

        {indexError && (
          <div className="mb-2 shrink-0 px-2 text-xs leading-relaxed">
            <p className="text-muted-foreground break-words" role="alert">{indexError}</p>
            <button type="button" className="focus-visible:ring-ring mt-1 rounded underline underline-offset-4 outline-none focus-visible:ring-2" onClick={refresh}>Retry</button>
          </div>
        )}

        <nav aria-label="Conversations" aria-busy={isLoading} className="min-h-0 max-h-[55svh] flex-1 overflow-y-auto overscroll-contain md:max-h-none">
          {sections.map((section) => {
            const sectionKey = `${group}:${filter}:${section.key}`;
            const isCollapsed = !query.trim() && !!collapsed[sectionKey];
            const visibleCount = group === "machine" && !query.trim() ? visibleCounts[sectionKey] ?? 5 : section.conversations.length;
            const hiddenCount = Math.max(0, section.conversations.length - visibleCount);
            const remaining = hiddenCount + section.pages.reduce((total, page) => total + Math.max(0, page.total - page.held), 0);
            return (
              <section key={section.key} data-slot="app-conversation-group" className="mb-2 last:mb-0" aria-label={section.label}>
                {section.label && (
                  <h3 className="mb-1">
                    <button type="button" data-slot="app-conversation-group-toggle" aria-expanded={!isCollapsed} className="text-foreground/80 hover:bg-foreground/5 focus-visible:ring-ring flex w-full min-w-0 items-center gap-1.5 rounded-md px-1 py-1.5 text-left text-[11px] font-semibold outline-none focus-visible:ring-2 focus-visible:ring-inset" onClick={() => setCollapsed((current) => ({ ...current, [sectionKey]: !isCollapsed }))}>
                      <ChevronDown aria-hidden="true" className={cn("size-3.5 shrink-0", isCollapsed && "-rotate-90")} />
                      <span className="min-w-0 truncate" title={section.label}>{section.label}</span>
                    </button>
                  </h3>
                )}
                <div hidden={isCollapsed} className={cn(section.label && "border-border/60 ml-2 border-l pl-2")}>
                  <ul className="space-y-0.5">
                    {section.conversations.slice(0, visibleCount).map((conversation) => {
                      const isActive = sameConversation(conversation, active);
                      const age = conversationAge(conversation.lastActivity, now);
                      const status = conversation.pendingQuestion ? "Needs your answer" : conversation.running ? "Running" : conversation.awaiting ? "Awaiting" : null;
                      return (
                        <li key={conversationHref(conversation)}>
                          <a onContextMenu={(event) => {
                            event.preventDefault(); setMenu(null);
                            setContextTarget({ conversation, x: event.clientX, y: event.clientY, trigger: event.currentTarget });
                          }} onKeyDown={(event) => {
                            if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
                              event.preventDefault(); setMenu(null);
                              const bounds = event.currentTarget.getBoundingClientRect();
                              setContextTarget({ conversation, x: bounds.left, y: bounds.bottom, trigger: event.currentTarget });
                            }
                          }} href={conversationHref(conversation)} data-slot="app-conversation-link" data-source={conversation.source ?? ""} data-agent={conversation.agent} data-run={conversation.runId} aria-current={isActive ? "page" : undefined} title={conversation.title} className={cn("focus-visible:ring-ring block rounded-md px-2 py-1.5 outline-none transition-colors focus-visible:ring-2 focus-visible:ring-inset", isActive ? "bg-foreground/8 text-foreground" : "text-muted-foreground hover:bg-foreground/5 hover:text-foreground")} onClick={(event) => {
                            if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
                            event.preventDefault();
                            openConversation(conversation);
                            setExpanded(false);
                          }}>
                            <span className="flex min-w-0 items-center gap-2">
                              <span className="min-w-0 flex-1 truncate text-xs font-medium leading-5">{conversation.title}</span>
                              {conversation.starred && <Star aria-label="Starred chat" className="size-3 shrink-0 fill-current" />}
                              {status && <span className={cn("size-1.5 shrink-0 rounded-full", conversation.pendingQuestion ? "bg-amber-600 dark:bg-amber-400" : conversation.running ? "bg-blue-600 dark:bg-blue-400" : "bg-foreground/50")} aria-hidden="true" />}
                            </span>
                            <span className="text-muted-foreground flex min-w-0 items-center gap-2 text-[11px] leading-4">
                              <span className="min-w-0 flex-1 truncate">{conversation.agentDisplayName ?? conversation.agent}{sourceCount > 1 && group !== "machine" && <span data-slot="app-conversation-machine"> · {conversation.source ?? "This machine"}</span>}</span>
                              {age && <time data-slot="app-conversation-time" className="shrink-0" dateTime={new Date(conversation.lastActivity).toISOString()} title={`Last activity: ${new Date(conversation.lastActivity).toLocaleString()}`} aria-label={`Last activity: ${age}`}>{age}</time>}
                            </span>
                            {status && <span className="sr-only">{status}</span>}
                          </a>
                        </li>
                      );
                    })}
                  </ul>
                  {remaining > 0 && !query.trim() && <button type="button" className="text-muted-foreground focus-visible:ring-ring hover:text-foreground mt-1 rounded px-2 py-1.5 text-left text-[11px] outline-none focus-visible:ring-2" data-slot="app-conversation-group-more" onClick={() => {
                    if (group === "machine") setVisibleCounts((current) => ({ ...current, [sectionKey]: visibleCount + 5 }));
                    if (hiddenCount === 0) section.pages.forEach((page) => { if (page.total > page.held) loadMore(page.source, page.project); });
                  }}>Load more · {remaining} older</button>}
                </div>
              </section>
            );
          })}
          {count === 0 && (
            <p className="text-muted-foreground px-2 py-3 text-xs leading-relaxed" role={isLoading ? "status" : undefined}>
              {isLoading && conversations.length === 0 ? "Loading conversations…" : query.trim() ? "No conversations match your search." : indexError ? "Conversations are unavailable." : filter === "all" ? "No conversations yet. Start a new chat." : "No sessions match this filter."}
              {!isLoading && !query.trim() && filter !== "all" && <button type="button" className="focus-visible:ring-ring mt-1 block rounded underline underline-offset-4 outline-none focus-visible:ring-2" onClick={() => filterRef.current?.click()}>Change filter</button>}
            </p>
          )}
        </nav>
      </div>
      {contextTarget && <ChatContextMenu key={conversationHref(contextTarget.conversation)} target={contextTarget} onClose={() => setContextTarget(null)} />}
      {menu && <SidebarMenu menu={menu} anchor={menu === "filters" ? filterRef : sourceRef} onClose={closeMenu} />}
    </aside>
  );
}
