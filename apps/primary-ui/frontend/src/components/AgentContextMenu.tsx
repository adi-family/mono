import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Pencil, Star } from "lucide-react";
import type { Agent } from "../lib/app-chat";
import { useAppChat } from "../lib/app-chat-state";

export type AgentContextTarget = { agent: Agent; x: number; y: number; trigger: HTMLElement };
export function AgentContextMenu({ target, onClose }: { target: AgentContextTarget; onClose: () => void }) {
  const { setAgentOverride } = useAppChat();
  const element = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: target.x, top: target.y });
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState(target.agent.displayName ?? target.agent.name);
  const [error, setError] = useState("");
  function close(restore = true) { if (restore && target.trigger.isConnected) target.trigger.focus(); onClose(); }
  useLayoutEffect(() => {
    const bounds = element.current?.getBoundingClientRect();
    if (bounds) setPosition({ left: Math.max(8, Math.min(target.x, innerWidth - bounds.width - 8)), top: Math.max(8, Math.min(target.y, innerHeight - bounds.height - 8)) });
    const input = element.current?.querySelector("input");
    if (input) { input.focus(); input.select(); } else element.current?.querySelector("button")?.focus();
  }, [target, renaming]);
  useEffect(() => {
    function outside(event: PointerEvent) { if (event.target instanceof Node && !element.current?.contains(event.target)) close(false); }
    function dismiss() { close(false); }
    document.addEventListener("pointerdown", outside);
    document.addEventListener("scroll", dismiss, true);
    window.addEventListener("resize", dismiss);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("scroll", dismiss, true); window.removeEventListener("resize", dismiss); };
  });
  return createPortal(<div ref={element} role={renaming ? "dialog" : "menu"} aria-label="Personal agent preferences" data-slot="agent-context-menu" style={position} className="bg-popover text-popover-foreground border-border fixed z-50 w-64 rounded-md border p-1 shadow-md" onContextMenu={(event) => event.preventDefault()} onKeyDown={(event) => {
    if (event.key === "Escape" || (!renaming && event.key === "Tab")) { event.preventDefault(); event.stopPropagation(); close(); }
    if (!renaming && ["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) { event.preventDefault();
      const buttons = Array.from(element.current?.querySelectorAll<HTMLButtonElement>('button[role="menuitem"]') ?? []);
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      buttons[event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : (index + (event.key === "ArrowUp" ? -1 : 1) + buttons.length) % buttons.length]?.focus(); }
  }}>
    {renaming ? <form className="space-y-2 p-2" onSubmit={(event) => {
      event.preventDefault();
      try { setAgentOverride(target.agent, "displayName", name.trim() && name.trim() !== target.agent.name ? name.trim() : undefined); close(); }
      catch { setError("Could not save this preference. Check browser storage and try again."); }
    }}>
      <label className="block text-xs font-medium">Name for me<input aria-label="Name for me" className="bg-background border-input mt-1 w-full rounded border px-2 py-1.5 text-sm" value={name} onChange={(event) => setName(event.target.value)} /></label>
      <p className="text-muted-foreground text-xs">Leave empty to restore {target.agent.name}.</p>
      <div className="flex gap-2"><button type="submit" className="hover:bg-accent rounded px-2 py-1 text-xs">Save</button><button type="button" className="hover:bg-accent rounded px-2 py-1 text-xs" onClick={() => close()}>Cancel</button></div>
    </form> : <>
    <button role="menuitem" type="button" className="hover:bg-accent focus:bg-accent flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs outline-none" onClick={() => {
      try { setAgentOverride(target.agent, "starred", target.agent.starred ? false : undefined); close(); }
      catch { setError("Could not save this preference. Check browser storage and try again."); }
    }}><Star aria-hidden="true" className="size-3.5" />{target.agent.starred ? "Unstar for me" : "Restore star for me"}</button>
    <button role="menuitem" type="button" className="hover:bg-accent focus:bg-accent flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs outline-none" onClick={() => { setError(""); setRenaming(true); }}><Pencil aria-hidden="true" className="size-3.5" />Rename for me</button>
    </>}
    <p className="text-muted-foreground px-2 py-1 text-xs">Only affects this browser.</p>
    {error && <p role="alert" className="text-destructive px-2 py-1 text-xs">{error}</p>}
  </div>, document.body);
}
