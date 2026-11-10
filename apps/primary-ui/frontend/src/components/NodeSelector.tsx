import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { Check, LockKeyhole, Network } from "lucide-react";
import { useLocation } from "react-router-dom";

import { Button, cn } from "@design-system/ui";

import { useChatSelection } from "../lib/app-chat-selection";
import { appHref } from "../lib/app-api";
import { hasSource, selectedSources, sourceName } from "../lib/app-chat-sources";
import { useMarketplaceSource } from "../lib/marketplace-source";
import { useIsMobile } from "../lib/use-is-mobile";

const menuItemClass = "focus:bg-accent hover:bg-accent flex min-h-8 w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs outline-none disabled:opacity-50 disabled:hover:bg-transparent";

/** What the machine choice drives on the current page. The chat list merges sessions from any
 * number of machines; a marketplace, with its sources and installs, is one machine's at a time. */
type Selection = {
  heading: string;
  label: string;
  multiple: boolean;
  chosen: (string | undefined)[];
  isChosen: (source?: string) => boolean;
  choose: (source?: string) => void;
};

function useSelection(): Selection {
  const { pathname } = useLocation();
  const { sources, toggleSource } = useChatSelection();
  const [marketplaceSource, setMarketplaceSource] = useMarketplaceSource();
  if (pathname === "/marketplace" || pathname.startsWith("/marketplace/")) {
    return {
      heading: "Marketplace on", label: "Marketplace machine", multiple: false,
      chosen: [marketplaceSource],
      isChosen: (source) => source === marketplaceSource,
      choose: setMarketplaceSource,
    };
  }
  return {
    heading: "Sessions from", label: "Conversation sources", multiple: true,
    chosen: selectedSources(sources),
    isChosen: (source) => hasSource(sources, source),
    choose: toggleSource,
  };
}

function NodeSelectorMenu({ anchor, selection, onClose }: { anchor: RefObject<HTMLButtonElement | null>; selection: Selection; onClose: (restoreFocus?: boolean) => void }) {
  const { fleetNodes, fleetLoading, fleetError } = useChatSelection();
  const element = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: 0, top: 0 });
  const role = selection.multiple ? "menuitemcheckbox" : "menuitemradio";
  const onlyLocal = selection.multiple && selection.isChosen() && selection.chosen.length === 1;
  // Chosen machines remain listed, and removable, if the fleet is offline or they were unpaired.
  const stored = selection.chosen.filter((source): source is string => !!source && !fleetNodes.some((node) => node.node === source));
  const pick = (source?: string) => {
    selection.choose(source);
    if (!selection.multiple) onClose();
  };

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
      data-slot="node-selector-menu"
      role="menu"
      aria-label={selection.label}
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
      <p className="text-muted-foreground px-2 py-1.5 text-[11px] font-medium">{selection.heading}</p>
      <button type="button" role={role} aria-checked={selection.isChosen()} title={onlyLocal ? "Keep at least one source selected." : undefined} className={menuItemClass} onClick={() => pick()}>
        <Check aria-hidden="true" className={cn("size-3.5 shrink-0", !selection.isChosen() && "invisible")} />
        This machine
      </button>
      {fleetNodes.map((node) => {
        const selected = selection.isChosen(node.node);
        return (
          <button key={node.node} type="button" role={role} aria-checked={selected} disabled={node.locked && !selected} title={node.locked && !selected ? "Unlock this source on the Fleet page first." : undefined} className={menuItemClass} onClick={() => pick(node.node)}>
            <Check aria-hidden="true" className={cn("size-3.5 shrink-0", !selected && "invisible")} />
            <span className="min-w-0 flex-1 truncate">{node.node}</span>
            {node.locked && <LockKeyhole aria-hidden="true" className="text-muted-foreground size-3 shrink-0" />}
          </button>
        );
      })}
      {stored.map((source) => (
        <button key={source} type="button" role={role} aria-checked="true" className={menuItemClass} onClick={() => pick(source)}>
          <Check aria-hidden="true" className="size-3.5 shrink-0" />
          <span className="min-w-0 truncate">{source}</span>
        </button>
      ))}
      {fleetLoading && <p role="status" className="text-muted-foreground px-2 py-2 text-xs">Loading sources…</p>}
      {fleetError && <p role="alert" className="text-muted-foreground px-2 py-2 text-xs leading-relaxed">{fleetError}</p>}
      {!fleetLoading && !fleetError && fleetNodes.length === 0 && <p className="text-muted-foreground px-2 py-2 text-xs leading-relaxed">No paired nodes yet.</p>}
      <div role="separator" className="bg-border my-1 h-px" />
      <a role="menuitem" href={appHref("/extended/settings/fleet")} className={menuItemClass}>Manage sources in Fleet</a>
    </div>,
    document.body,
  );
}

/** Which machine the page is looking at — in every page's topbar, since it decides what the
 * page shows: whose sessions the chat lists, whose marketplace is being browsed. The topbar is
 * hidden below the mobile breakpoint on the chat page (see index.css), so `hideOnMobile` drops
 * that copy there and `mobileOnly` renders the same control inside the chat sidebar's own header. */
export function NodeSelector({ mobileOnly = false, hideOnMobile = false }: { mobileOnly?: boolean; hideOnMobile?: boolean }) {
  const isMobile = useIsMobile();
  const selection = useSelection();
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  if (mobileOnly ? !isMobile : isMobile && hideOnMobile) return null;

  const names = selection.chosen.map(sourceName);
  const count = names.length;
  const shared = {
    ref: anchor, "data-slot": "node-selector", "aria-label": selection.label, title: `${selection.heading} ${names.join(", ")}`,
    "aria-haspopup": "menu" as const, "aria-expanded": open, onClick: () => setOpen((current) => !current),
  };

  return (
    <>
      {mobileOnly ? (
        <Button {...shared} variant="ghost" size="icon" className={cn("size-7 shrink-0", count > 1 && "w-auto min-w-7 gap-1 px-1.5", count > 1 || !selection.isChosen() ? "text-foreground" : "text-muted-foreground")}>
          <Network aria-hidden="true" className="size-3.5" />
          {count > 1 && <span className="text-[10px]">{count}</span>}
        </Button>
      ) : (
        <Button {...shared} variant="ghost" size="sm" className="h-7 shrink-0 gap-1.5 px-2 text-xs">
          <Network aria-hidden="true" className="size-3.5" />
          <span className="max-w-32 truncate">{count === 1 ? names[0] : `${count} machines`}</span>
        </Button>
      )}
      {open && <NodeSelectorMenu anchor={anchor} selection={selection} onClose={(restoreFocus = true) => {
        if (restoreFocus) anchor.current?.focus();
        setOpen(false);
      }}/>}
    </>
  );
}
