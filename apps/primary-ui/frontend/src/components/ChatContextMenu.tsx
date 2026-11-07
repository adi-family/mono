import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { EyeOff, Pencil, Star } from "lucide-react";
import { Button } from "@design-system/ui";
import type { Conversation } from "../lib/app-chat";
import { useAppChat } from "../lib/app-chat-state";

export type ChatContextTarget = { conversation: Conversation; x: number; y: number; trigger: HTMLElement };
export function ChatContextMenu({ target, onClose }: { target: ChatContextTarget; onClose: () => void }) {
  const { renameChat, starChat, hideChat } = useAppChat();
  const element = useRef<HTMLDivElement>(null);
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(target.conversation.title);
  const [position, setPosition] = useState({ left: target.x, top: target.y });
  function close(restore = true) {
    if (restore && target.trigger.isConnected) target.trigger.focus();
    onClose();
  }
  useLayoutEffect(() => {
    const bounds = element.current?.getBoundingClientRect();
    if (bounds) setPosition({ left: Math.max(8, Math.min(target.x, window.innerWidth - bounds.width - 8)), top: Math.max(8, Math.min(target.y, window.innerHeight - bounds.height - 8)) });
    const input = element.current?.querySelector("input");
    if (input) { input.focus(); input.select(); }
    else element.current?.querySelector("button")?.focus();
  }, [target, editing]);
  useEffect(() => {
    function outside(event: PointerEvent) { if (event.target instanceof Node && !element.current?.contains(event.target)) close(false); }
    function dismiss() { close(false); }
    document.addEventListener("pointerdown", outside);
    window.addEventListener("resize", dismiss);
    document.addEventListener("scroll", dismiss, true);
    return () => { document.removeEventListener("pointerdown", outside); window.removeEventListener("resize", dismiss); document.removeEventListener("scroll", dismiss, true); };
  });
  function save(action: () => Promise<void>) {
    const result = action();
    close();
    void result;
  }
  const itemClass = "hover:bg-accent focus:bg-accent flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs outline-none disabled:opacity-50";
  return createPortal(<div ref={element} data-slot="chat-context-menu" role={editing ? "dialog" : "menu"} aria-label={editing ? "Rename chat" : "Chat actions"} style={position} className="bg-popover text-popover-foreground border-border fixed z-50 w-64 rounded-md border p-1 shadow-md" onContextMenu={(event) => event.preventDefault()} onKeyDown={(event) => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); }
    if (["ArrowDown", "ArrowUp", "Home", "End", "Tab"].includes(event.key)) {
      if (editing && event.key !== "Tab") return;
      const items = [...(element.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)') ?? [])];
      if (!items.length) return;
      event.preventDefault();
      const current = items.indexOf(document.activeElement as HTMLElement);
      const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (current + (event.key === "ArrowUp" || (event.key === "Tab" && event.shiftKey) ? -1 : 1) + items.length) % items.length;
      items[next]?.focus();
    }
  }}>
    {editing ? <form className="space-y-2 p-2" onSubmit={(event) => { event.preventDefault(); void save(() => renameChat(target.conversation, name)); }}>
      <label className="block text-xs font-medium" htmlFor="context-chat-name">Chat name</label>
      <input id="context-chat-name" value={name} onChange={(event) => setName(event.target.value)} className="border-input focus-visible:ring-ring w-full rounded-md border px-2 py-1 text-sm outline-none focus-visible:ring-2" />
      <p className="text-muted-foreground text-[11px]">Leave empty to restore the default title.</p>
      <div className="flex justify-end gap-1">
        <Button type="button" size="sm" variant="ghost" onClick={() => close()}>Cancel</Button>
        <Button type="submit" size="sm">Save</Button>
      </div>
    </form> : <>
      <button role="menuitem" type="button" className={itemClass} onClick={() => setEditing(true)}><Pencil aria-hidden="true" className="size-3.5" />Rename</button>
      <button role="menuitem" type="button" className={itemClass} onClick={() => void save(() => starChat(target.conversation, !target.conversation.starred))}><Star aria-hidden="true" className={target.conversation.starred ? "size-3.5 fill-current" : "size-3.5"} />{target.conversation.starred ? "Unstar" : "Star"}</button>
      <button role="menuitem" type="button" className={itemClass} onClick={() => void save(() => hideChat(target.conversation))}><EyeOff aria-hidden="true" className="size-3.5" />Hide</button>
    </>}
  </div>, document.body);
}
