import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Star } from "lucide-react";
import type { Agent } from "../lib/app-chat";
import { useAppChat } from "../lib/app-chat-state";

export type AgentContextTarget = { agent: Agent; x: number; y: number; trigger: HTMLElement };
export function AgentContextMenu({ target, onClose }: { target: AgentContextTarget; onClose: () => void }) {
  const { setAgentOverride } = useAppChat();
  const element = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: target.x, top: target.y });
  const [error, setError] = useState("");
  function close(restore = true) { if (restore && target.trigger.isConnected) target.trigger.focus(); onClose(); }
  useLayoutEffect(() => {
    const bounds = element.current?.getBoundingClientRect();
    if (bounds) setPosition({ left: Math.max(8, Math.min(target.x, innerWidth - bounds.width - 8)), top: Math.max(8, Math.min(target.y, innerHeight - bounds.height - 8)) });
    element.current?.querySelector("button")?.focus();
  }, [target]);
  useEffect(() => {
    function outside(event: PointerEvent) { if (event.target instanceof Node && !element.current?.contains(event.target)) close(false); }
    function dismiss() { close(false); }
    document.addEventListener("pointerdown", outside);
    document.addEventListener("scroll", dismiss, true);
    window.addEventListener("resize", dismiss);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("scroll", dismiss, true); window.removeEventListener("resize", dismiss); };
  });
  return createPortal(<div ref={element} role="menu" aria-label="Personal agent preferences" data-slot="agent-context-menu" style={position} className="bg-popover text-popover-foreground border-border fixed z-50 w-64 rounded-md border p-1 shadow-md" onContextMenu={(event) => event.preventDefault()} onKeyDown={(event) => {
    if (event.key === "Escape" || event.key === "Tab") { event.preventDefault(); event.stopPropagation(); close(); }
    if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) { event.preventDefault(); element.current?.querySelector("button")?.focus(); }
  }}>
    <button role="menuitem" type="button" className="hover:bg-accent focus:bg-accent flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs outline-none" onClick={() => {
      try { setAgentOverride(target.agent, "starred", target.agent.starred ? false : undefined); close(); }
      catch { setError("Could not save this preference. Check browser storage and try again."); }
    }}><Star aria-hidden="true" className="size-3.5" />{target.agent.starred ? "Unstar for me" : "Restore star for me"}</button>
    <p className="text-muted-foreground px-2 py-1 text-xs">Only affects this browser.</p>
    {error && <p role="alert" className="text-destructive px-2 py-1 text-xs">{error}</p>}
  </div>, document.body);
}
