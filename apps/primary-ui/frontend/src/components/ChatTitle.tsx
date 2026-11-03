import { useEffect, useRef, useState, type FormEvent } from "react";
import { Check, LoaderCircle, Pencil, X } from "lucide-react";
import { Button } from "@design-system/ui";
import type { ConversationRef } from "../lib/app-chat";

export function ChatTitle({ title, conversation, onRename }: {
  title: string; conversation: ConversationRef | null;
  onRename: (conversation: ConversationRef, title: string) => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(title);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
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
    {conversation && <Button ref={trigger} type="button" size="icon" variant="ghost" className="text-muted-foreground size-6 shrink-0" aria-label="Rename chat" onClick={() => { setName(title); setEditing(true); }}><Pencil aria-hidden="true" className="size-3" /></Button>}
  </div>;
}
