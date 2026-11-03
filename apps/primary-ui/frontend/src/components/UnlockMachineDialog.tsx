import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { LoaderCircle, LockKeyhole } from "lucide-react";

import { Button } from "@design-system/ui";

export function UnlockMachineDialog({ machine, onUnlock, onClose }: {
  machine: string;
  onUnlock: (username: string, password: string, signal: AbortSignal) => Promise<void>;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const requestRef = useRef<AbortController | null>(null);
  const id = useId();
  const [username, setUsername] = useState("adi");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog?.showModal();
    dialog?.querySelector<HTMLInputElement>('input[type="password"]')?.focus();
    return () => {
      requestRef.current?.abort();
      dialog?.close();
      const fallback = document.querySelector<HTMLElement>(`[data-slot="home-machine-filter"][data-machine="${CSS.escape(machine)}"]`);
      (previousFocus?.isConnected ? previousFocus : fallback)?.focus({ preventScroll: true });
    };
  }, []);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (requestRef.current || !password.length) return;
    const attempt = new AbortController();
    requestRef.current = attempt;
    setBusy(true);
    setError(null);
    try {
      await onUnlock(username, password, attempt.signal);
      if (!attempt.signal.aborted) { setPassword(""); onClose(); }
    } catch (error) {
      if (!attempt.signal.aborted) setError(error instanceof Error ? error.message : "This machine could not be unlocked. Try again.");
    } finally {
      if (!attempt.signal.aborted) { requestRef.current = null; setBusy(false); }
    }
  }

  return (
    <dialog
      ref={dialogRef}
      data-slot="unlock-machine-dialog"
      aria-label={`Unlock ${machine}`}
      aria-describedby={`${id}-description`}
      className="bg-popover text-popover-foreground fixed top-1/2 left-1/2 m-0 w-[calc(100vw_-_2rem)] max-w-sm -translate-x-1/2 -translate-y-1/2 rounded-xl border p-0 shadow-xl backdrop:bg-black/50"
      onCancel={(event) => { event.preventDefault(); onClose(); }}
    >
      <form onSubmit={submit} aria-busy={busy} className="space-y-5 p-5 sm:p-6">
        <header>
          <LockKeyhole className="text-muted-foreground mb-3 size-5" aria-hidden="true" />
          <h2 className="text-lg font-semibold [overflow-wrap:anywhere]">Unlock {machine}</h2>
          <p id={`${id}-description`} className="text-muted-foreground mt-1.5 text-sm leading-5">Enter the machine’s credentials to load its apps. ADI remembers them until you lock the machine again.</p>
        </header>
        <div className="space-y-4">
          <div className="space-y-1.5">
            <label htmlFor={`${id}-username`} className="text-sm font-medium">Username</label>
            <input id={`${id}-username`} aria-label="Username" type="text" autoComplete="username" value={username} disabled={busy} onChange={(event) => setUsername(event.target.value)} className="border-input bg-background focus-visible:ring-ring h-10 w-full rounded-md border px-3 text-sm outline-none focus-visible:ring-2 disabled:opacity-50" />
          </div>
          <div className="space-y-1.5">
            <label htmlFor={`${id}-password`} className="text-sm font-medium">Password</label>
            <input id={`${id}-password`} aria-label="Password" type="password" autoComplete="new-password" required value={password} disabled={busy} onChange={(event) => setPassword(event.target.value)} aria-invalid={!!error} aria-describedby={error ? `${id}-error` : undefined} className="border-input bg-background focus-visible:ring-ring h-10 w-full rounded-md border px-3 text-sm outline-none focus-visible:ring-2 disabled:opacity-50" />
          </div>
        </div>
        {error && <p id={`${id}-error`} role="alert" className="text-destructive text-sm leading-5 [overflow-wrap:anywhere]">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="outline" onClick={onClose}>Cancel</Button>
          <Button type="submit" disabled={busy || !password.length} disabledReason={busy ? "The machine is being unlocked." : "Enter the machine password."}>
            {busy && <LoaderCircle className="size-4 motion-safe:animate-spin" aria-hidden="true" />}
            {busy ? "Unlocking…" : "Unlock machine"}
          </Button>
        </div>
      </form>
    </dialog>
  );
}
