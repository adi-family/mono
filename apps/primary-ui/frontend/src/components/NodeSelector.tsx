import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { Check, LockKeyhole, Network } from "lucide-react";

import { Button, cn } from "@design-system/ui";

import { useAppChat } from "../lib/app-chat-state";
import { appHref } from "../lib/app-api";

const menuItemClass = "focus:bg-accent hover:bg-accent flex min-h-8 w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs outline-none disabled:opacity-50 disabled:hover:bg-transparent";
// Matches index.css's own mobile breakpoint, where the topbar this control normally sits in is
// hidden on `/interface` in favor of each screen's own header.
const mobileQuery = "(max-width: 767px)";

function useIsMobile() {
  const [mobile, setMobile] = useState(() => window.matchMedia(mobileQuery).matches);
  useEffect(() => {
    const media = window.matchMedia(mobileQuery);
    const update = () => setMobile(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  return mobile;
}

function NodeSelectorMenu({ anchor, onClose }: { anchor: RefObject<HTMLButtonElement | null>; onClose: (restoreFocus?: boolean) => void }) {
  const { sources, fleetNodes, fleetLoading, fleetError, toggleSource } = useAppChat();
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
      data-slot="node-selector-menu"
      role="menu"
      aria-label="Conversation sources"
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
    </div>,
    document.body,
  );
}

/** Which machines' sessions the chat interface reads from — lives in the topbar since it
 * affects every source on the page, not just the sidebar's own list. The topbar itself is
 * hidden below the mobile breakpoint on `/interface` (see index.css), so `mobileOnly` renders
 * this same control inside the sidebar's own header there instead of leaving it unreachable. */
export function NodeSelector({ mobileOnly = false }: { mobileOnly?: boolean }) {
  const isMobile = useIsMobile();
  const { sources } = useAppChat();
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const sourceCount = Number(sources.local) + sources.nodes.length;
  if (isMobile !== mobileOnly) return null;

  return (
    <>
      <Button
        ref={anchor}
        variant="ghost"
        size="icon"
        className={cn("size-7 shrink-0", sourceCount > 1 && "w-auto min-w-7 gap-1 px-1.5", sourceCount > 1 || !sources.local ? "text-foreground" : "text-muted-foreground")}
        aria-label="Conversation sources"
        title={`Sessions from ${[...(sources.local ? ["This machine"] : []), ...sources.nodes].join(", ")}`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <Network aria-hidden="true" className="size-3.5" />
        {sourceCount > 1 && <span className="text-[10px]">{sourceCount}</span>}
      </Button>
      {open && <NodeSelectorMenu anchor={anchor} onClose={(restoreFocus = true) => {
        if (restoreFocus) anchor.current?.focus();
        setOpen(false);
      }}/>}
    </>
  );
}
