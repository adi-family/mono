import { useEffect, useId, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, Filter, Plus, Search, Star } from "lucide-react";

import { Button, cn } from "@design-system/ui";

import { useChatSelection } from "../lib/app-chat-selection";
import { useChatList } from "../lib/app-chat-list";
import { useChatThread } from "../lib/app-chat-thread";
import { conversationAge, conversationFilters, conversationGroups, conversationSections } from "../lib/app-chat-sidebar";
import { sameConversation } from "../lib/app-chat-sources";
import { conversationHref } from "../lib/app-chat-transcript";
import { setEventLogOpen } from "../lib/event-log";

import { ChatContextMenu, type ChatContextTarget } from "./ChatContextMenu";
import { NodeSelector } from "./NodeSelector";

const menuItemClass = "focus:bg-accent hover:bg-accent flex min-h-8 w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs outline-none disabled:opacity-50 disabled:hover:bg-transparent";

function FilterMenu({ anchor, onClose }: { anchor: RefObject<HTMLButtonElement | null>; onClose: (restoreFocus?: boolean) => void }) {
  const { filter, setFilter, group, setGroup } = useChatSelection();
  const element = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: 0, top: 0 });

  useLayoutEffect(() => {
    const bounds = anchor.current?.getBoundingClientRect();
    if (bounds) setPosition({ left: Math.max(8, Math.min(bounds.left, window.innerWidth - 248)), top: bounds.bottom + 5 });
    const selected = element.current?.querySelector<HTMLButtonElement>('[aria-checked="true"]:not(:disabled)')
      ?? element.current?.querySelector<HTMLButtonElement>('[role^="menuitem"]:not(:disabled)');
    selected?.focus();
  }, [anchor]);

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
      aria-label="Conversation filters"
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
    </div>,
    document.body,
  );
}

export function AppChatSidebar() {
  const { active, sources, filter, group, openConversation, newConversation } = useChatSelection();
  const { conversations, isLoading, indexFailing, sourceStates, loadMore } = useChatList();
  const { refresh } = useChatThread();
  const [contextTarget, setContextTarget] = useState<ChatContextTarget | null>(null);
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [visibleCounts, setVisibleCounts] = useState<Record<string, number>>({});
  const [filterMenuOpen, setFilterMenuOpen] = useState(false);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);
  const filterRef = useRef<HTMLButtonElement>(null);
  const navigationId = useId();
  const sections = conversationSections(conversations, sourceStates, group, query);
  const count = sections.reduce((total, section) => total + section.conversations.length, 0);
  const sourceCount = Number(sources.local) + sources.nodes.length;
  const currentFilter = conversationFilters.find((option) => option.value === filter)?.label;
  const closeFilterMenu = (restoreFocus = true) => {
    if (restoreFocus) filterRef.current?.focus();
    setFilterMenuOpen(false);
  };

  return (
    <aside data-slot="app-chat-sidebar" className="flex h-full min-h-0 min-w-0 flex-col">
      <div id={navigationId} className="flex min-h-0 flex-1 flex-col">
        <div className="mb-2 flex shrink-0 items-center gap-1 px-1 pt-3 md:pt-0">
          <h2 className="mr-auto pl-1 text-sm font-semibold">Chats</h2>
          <NodeSelector mobileOnly/>
          <Button ref={filterRef} variant="ghost" size="icon" className={cn("size-7 shrink-0", filter === "all" ? "text-muted-foreground" : "text-foreground bg-foreground/8")} aria-label="Filter conversations" title={currentFilter} aria-haspopup="menu" aria-expanded={filterMenuOpen} onClick={() => setFilterMenuOpen((current) => !current)}>
            <Filter aria-hidden="true" className="size-3.5" />
          </Button>
          <Button variant="ghost" size="icon" className="text-muted-foreground size-7 shrink-0 hover:text-foreground" aria-label="New conversation" title="New conversation" onClick={() => newConversation()}>
            <Plus aria-hidden="true" className="size-4" />
          </Button>
        </div>

        <div className="relative mb-2 shrink-0">
          <Search aria-hidden="true" className="text-muted-foreground pointer-events-none absolute top-2.5 left-2.5 size-3.5" />
          <input type="search" aria-label="Search conversations" placeholder="Search chats" value={query} onChange={(event) => setQuery(event.target.value)} className="border-input/60 bg-background/40 placeholder:text-muted-foreground focus-visible:ring-ring h-8 w-full rounded-md border py-1 pr-2 pl-8 text-xs outline-none focus-visible:ring-2" />
        </div>

        {/* A machine that stops answering is reported in the event log; the list keeps what it has. */}
        <nav aria-label="Conversations" aria-busy={isLoading} className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
          {sections.map((section) => {
            const sectionKey = `${group}:${filter}:${section.key}`;
            const isCollapsed = !query.trim() && !!collapsed[sectionKey];
            const visibleCount = group === "machine" && !query.trim() ? visibleCounts[sectionKey] ?? 5 : section.conversations.length;
            const hiddenCount = Math.max(0, section.conversations.length - visibleCount);
            const remaining = hiddenCount + section.pages.reduce((total, page) => total + Math.max(0, page.total - page.loaded), 0);
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
                            event.preventDefault(); setFilterMenuOpen(false);
                            setContextTarget({ conversation, x: event.clientX, y: event.clientY, trigger: event.currentTarget });
                          }} onKeyDown={(event) => {
                            if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
                              event.preventDefault(); setFilterMenuOpen(false);
                              const bounds = event.currentTarget.getBoundingClientRect();
                              setContextTarget({ conversation, x: bounds.left, y: bounds.bottom, trigger: event.currentTarget });
                            }
                          }} href={conversationHref(conversation)} data-slot="app-conversation-link" data-source={conversation.source ?? ""} data-agent={conversation.agent} data-run={conversation.runId} aria-current={isActive ? "page" : undefined} title={conversation.title} className={cn("focus-visible:ring-ring block rounded-md px-2 py-1.5 outline-none transition-colors focus-visible:ring-2 focus-visible:ring-inset", isActive ? "bg-foreground/8 text-foreground" : "text-muted-foreground hover:bg-foreground/5 hover:text-foreground")} onClick={(event) => {
                            if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
                            event.preventDefault();
                            openConversation(conversation);
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
                    if (hiddenCount === 0) section.pages.forEach((page) => { if (page.total > page.loaded) loadMore(page.source, page.project); });
                  }}>Load more · {remaining} older</button>}
                </div>
              </section>
            );
          })}
          {count === 0 && (
            <p className="text-muted-foreground px-2 py-3 text-xs leading-relaxed" role={isLoading ? "status" : undefined}>
              {isLoading && conversations.length === 0 ? "Loading conversations…" : query.trim() ? "No conversations match your search." : indexFailing ? "Conversations are unavailable." : filter === "all" ? "No conversations yet. Start a new chat." : "No sessions match this filter."}
              {!isLoading && !query.trim() && indexFailing && <span className="mt-1 flex flex-wrap gap-x-3">
                <button type="button" className="focus-visible:ring-ring rounded underline underline-offset-4 outline-none focus-visible:ring-2" onClick={refresh}>Retry</button>
                <button type="button" data-slot="app-chat-open-event-log" className="focus-visible:ring-ring rounded underline underline-offset-4 outline-none focus-visible:ring-2" onClick={() => setEventLogOpen(true)}>Open event log</button>
              </span>}
              {!isLoading && !query.trim() && !indexFailing && filter !== "all" && <button type="button" className="focus-visible:ring-ring mt-1 block rounded underline underline-offset-4 outline-none focus-visible:ring-2" onClick={() => filterRef.current?.click()}>Change filter</button>}
            </p>
          )}
        </nav>
      </div>
      {contextTarget && <ChatContextMenu key={conversationHref(contextTarget.conversation)} target={contextTarget} onClose={() => setContextTarget(null)} />}
      {filterMenuOpen && <FilterMenu anchor={filterRef} onClose={closeFilterMenu} />}
    </aside>
  );
}
