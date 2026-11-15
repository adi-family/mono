import { useEffect, useRef, type ReactNode } from "react";

import { Button } from "@design-system/ui";

/** A modal question before an action that cannot be taken back. Escape and Cancel both decline. */
export function ConfirmDialog({ title, confirmLabel, busy, error, onConfirm, onClose, children }: {
  title: string;
  confirmLabel: string;
  busy: boolean;
  error?: string | null;
  onConfirm: () => void;
  onClose: () => void;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog?.showModal();
    return () => {
      dialog?.close();
      if (previous?.isConnected) previous.focus({ preventScroll: true });
    };
  }, []);
  const waiting = { disabled: busy, disabledReason: "Wait for the current action to finish." };

  return (
    <dialog
      ref={ref}
      data-slot="confirm-dialog"
      aria-label={title}
      onCancel={(event) => { event.preventDefault(); if (!busy) onClose(); }}
      className="bg-popover text-popover-foreground fixed top-1/2 left-1/2 m-0 max-h-[90dvh] w-[calc(100vw_-_2rem)] max-w-md -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-xl border p-6 shadow-xl backdrop:bg-black/50"
    >
      <h2 className="mb-3 text-lg font-semibold">{title}</h2>
      <div className="text-muted-foreground text-sm leading-relaxed">{children}</div>
      {error && <p role="alert" className="text-destructive mt-4 text-sm">{error}</p>}
      <div className="mt-6 flex justify-end gap-2">
        <Button type="button" variant="outline" {...waiting} onClick={onClose}>Cancel</Button>
        <Button type="button" variant="destructive" data-slot="confirm-dialog-confirm" {...waiting} onClick={onConfirm}>{busy ? "Working…" : confirmLabel}</Button>
      </div>
    </dialog>
  );
}
