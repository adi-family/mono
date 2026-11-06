import { useEffect, useRef, useState, type FormEvent } from "react";
import { Check, Copy, LoaderCircle, Pencil, Star, X } from "lucide-react";
import { Button } from "@design-system/ui";
import type { ConversationRef } from "../lib/app-chat";

export function ChatTitle({ title, conversation, onRename, starred, onStar }: {
  title: string; conversation: ConversationRef | null;
  starred: boolean; onStar: (conversation: ConversationRef, starred: boolean) => Promise<void>;
  onRename: (conversation: ConversationRef, title: string) => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(title);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [copiedId, setCopiedId] = useState("");
  useEffect(() => { setCopiedId(""); }, [conversation?.agent, conversation?.runId, conversation?.source]);
  useEffect(() => {
    if (!copiedId) return;
    const timer = window.setTimeout(() => setCopiedId(""), 2000);
    return () => window.clearTimeout(timer);
  }, [copiedId]);
  async function copyId() {
    if (!conversation) return;
    const id = conversation.runId;
    const fallback = () => {
      const previousFocus = document.activeElement;
      const input = document.createElement("textarea");
      input.value = id;
      input.style.position = "fixed";
      input.style.opacity = "0";
      input.setAttribute("aria-label", "Chat ID to copy");
      document.body.append(input);
      input.select();
      try {
        if (!document.execCommand("copy")) throw new Error("Copy failed");
      } finally {
        input.remove();
        if (previousFocus instanceof HTMLElement) previousFocus.focus({ preventScroll: true });
      }
    };
    setError("");
    try {
      if (navigator.clipboard) {
        try { await navigator.clipboard.writeText(id); } catch { fallback(); }
      } else { fallback(); }
      setCopiedId(id);
    } catch { setCopiedId(""); setError("Could not copy the chat ID. Try again."); }
  }
  const inFlight = useRef(false);
  const input = useRef<HTMLInputElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  useEffect(() => { if (editing) { input.current?.focus(); input.current?.select(); } }, [editing]);
  function close() { setEditing(false); setError(""); requestAnimationFrame(() => trigger.current?.focus()); }
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!conversation || inFlight.current) return;
    inFlight.current = true; setSaving(true); setError("");
    try { await onRename(conversation, name); close(); }
    catch (cause) { setError(cause instanceof Error ? cause.message : "Could not rename this chat. Try again."); }
    finally { inFlight.current = false; setSaving(false); }
  }
  return editing ? <form data-slot="chat-rename" aria-label="Rename chat" onSubmit={submit} className="min-w-0" onKeyDown={(event) => {
    if (event.key === "Escape" && !saving) { event.preventDefault(); close(); }
  }}>
    <div className="flex min-w-0 items-center gap-1">
      <input ref={input} aria-label="Chat name" value={name} disabled={saving} placeholder="Default chat title" onChange={(event) => setName(event.target.value)} className="border-input focus-visible:ring-ring min-w-0 flex-1 rounded-md border px-2 py-1 text-sm outline-none focus-visible:ring-2" />
      <Button type="submit" size="icon" variant="ghost" className="size-7 shrink-0" aria-label="Save chat name" disabled={saving} disabledReason="The chat name is being saved.">{saving ? <LoaderCircle aria-hidden="true" className="size-3.5 motion-safe:animate-spin" /> : <Check aria-hidden="true" className="size-3.5" />}</Button>
      <Button type="button" size="icon" variant="ghost" className="size-7 shrink-0" aria-label="Cancel rename" disabled={saving} disabledReason="The chat name is being saved." onClick={close}><X aria-hidden="true" className="size-3.5" /></Button>
    </div>
    <p className="text-muted-foreground mt-1 text-[11px]">Leave empty to restore the default title.</p>
    {error && <p role="alert" className="text-destructive mt-1 text-xs">{error}</p>}
  </form> : <div className="flex min-w-0 items-center gap-1">
    <h1 className="truncate text-sm font-medium" title={title}>{title}</h1>
    {conversation && <Button type="button" size="icon" variant="ghost" className="text-muted-foreground size-6 shrink-0" aria-label={starred ? "Unstar chat" : "Star chat"} aria-pressed={starred} disabled={saving} disabledReason="The chat is being updated." onClick={async () => {
      if (inFlight.current) return;
      inFlight.current = true; setSaving(true); setError("");
      try { await onStar(conversation, !starred); }
      catch (cause) { setError(cause instanceof Error ? cause.message : "Could not update this chat. Try again."); }
      finally { inFlight.current = false; setSaving(false); }
    }}><Star aria-hidden="true" className={starred ? "size-3 fill-current" : "size-3"} /></Button>}
    {conversation && <Button type="button" size="icon" variant="ghost" className="text-muted-foreground size-6 shrink-0" aria-label="Copy chat ID" title={`Copy chat ID: ${conversation.runId}`} onClick={copyId}>
      {copiedId === conversation.runId ? <Check aria-hidden="true" className="size-3" /> : <Copy aria-hidden="true" className="size-3" />}
    </Button>}
    <span role="status" className="sr-only">{copiedId && copiedId === conversation?.runId ? "Chat ID copied" : ""}</span>
    {error && <p role="alert" className="text-destructive text-xs">{error}</p>}
    {conversation && <Button ref={trigger} type="button" size="icon" variant="ghost" className="text-muted-foreground size-6 shrink-0" aria-label="Rename chat" onClick={() => { setName(title); setEditing(true); }}><Pencil aria-hidden="true" className="size-3" /></Button>}
  </div>;
}
