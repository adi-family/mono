import * as React from "react";
import { ArrowUp, LoaderCircle, Paperclip, Square, X } from "lucide-react";

import { cn } from "../lib/cn";
import { Button, type ButtonDisabledProps } from "./button";

type ChatComposerProps = Omit<React.ComponentProps<"form">, "onSubmit" | "onError" | "children"> &
  ButtonDisabledProps & {
    /** Controlled attachments; the host uploads files and supplies their preview URLs. */
    attachments?: readonly { id: string; name: string; url?: string; error?: string }[];
    onFilesSelect?: (files: File[]) => void;
    onAttachmentRemove?: (id: string) => void;
    attachmentsBusy?: boolean;
    value: string;
    onValueChange: (value: string) => void;
    /** Resolving accepts the message; rejecting keeps the draft and displays errorMessage. */
    onSend: (message: string) => void | Promise<void>;
    isGenerating?: boolean;
    onStop?: () => void;
    inputLabel?: string;
    placeholder?: string;
    sendLabel?: string;
    sendingLabel?: string;
    stopLabel?: string;
    errorMessage?: string;
    sendOnEnter?: boolean;
    /** Optional tools, such as attachment or model controls. */
    children?: React.ReactNode;
    textareaProps?: Omit<
      React.ComponentProps<"textarea">,
      "value" | "defaultValue" | "onChange" | "disabled" | "children" | "form"
    >;
  };

function ChatComposer({
  value,
  attachments = [],
  onFilesSelect,
  onAttachmentRemove,
  attachmentsBusy = false,
  onValueChange,
  onSend,
  isGenerating = false,
  onStop,
  disabled = false,
  disabledReason,
  inputLabel = "Message",
  placeholder = "Write a message…",
  sendLabel = "Send message",
  sendingLabel = "Sending message…",
  stopLabel = "Stop response",
  errorMessage = "Message could not be sent. Try again.",
  sendOnEnter = true,
  children,
  textareaProps = {},
  className,
  ...props
}: ChatComposerProps) {
  const id = React.useId();
  const fileInput = React.useRef<HTMLInputElement>(null);
  const [sending, setSending] = React.useState(false);
  const [failed, setFailed] = React.useState(false);
  const inFlight = React.useRef(false);
  const mounted = React.useRef(true);
  const currentValue = React.useRef(value);
  currentValue.current = value;

  React.useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  const reason = disabledReason?.trim();
  if (disabled && !reason) {
    throw new Error("A disabled ChatComposer requires a non-empty disabledReason.");
  }

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const draft = currentValue.current;
    const message = draft.trim();
    if (disabled || isGenerating || inFlight.current || attachmentsBusy || attachments.some((image) => image.error) || (!message && !attachments.length)) return;

    event.currentTarget.querySelector<HTMLTextAreaElement>("[data-slot=chat-composer-input]")?.focus({ preventScroll: true });
    inFlight.current = true;
    setSending(true);
    setFailed(false);
    currentValue.current = "";
    onValueChange("");
    try {
      await onSend(message);
    } catch {
      if (mounted.current) {
        if (!currentValue.current) { currentValue.current = draft; onValueChange(draft); }
        setFailed(true);
      }
    } finally {
      inFlight.current = false;
      if (mounted.current) setSending(false);
    }
  }

  const {
    className: inputClassName,
    onKeyDown,
    "aria-describedby": describedBy,
    ...inputProps
  } = textareaProps;
  const helperId = `${id}-help`;
  const errorId = `${id}-error`;
  const disabledId = `${id}-disabled`;
  const cannotSend = disabled || sending || attachmentsBusy || attachments.some((image) => image.error) || (!value.trim() && !attachments.length);
  const sendDisabledReason = disabled
    ? reason!
    : attachmentsBusy ? "Wait for images to finish uploading."
    : attachments.some((image) => image.error) ? "Remove failed attachments before sending."
    : sending
      ? "Wait for this message to finish sending."
      : "Write a message before sending it.";

  return (
    <form
      data-slot="chat-composer"
      className={cn("bg-background flex min-w-0 flex-col gap-2 rounded-xl border p-3", className)}
      {...props}
      aria-busy={sending || isGenerating || undefined}
      onSubmit={submit}
      onPaste={(event) => {
        props.onPaste?.(event);
        if (event.defaultPrevented || disabled || sending || !onFilesSelect) return;
        const files = Array.from(event.clipboardData.files).filter((file) => file.type.startsWith("image/"));
        if (files.length) { event.preventDefault(); onFilesSelect(files); }
      }}
      onDragOver={(event) => {
        props.onDragOver?.(event);
        if (!event.defaultPrevented && onFilesSelect && Array.from(event.dataTransfer.types).includes("Files")) event.preventDefault();
      }}
      onDrop={(event) => {
        props.onDrop?.(event);
        if (event.defaultPrevented || !onFilesSelect) return;
        if (event.dataTransfer.files.length) {
          event.preventDefault();
          if (!disabled && !sending) onFilesSelect(Array.from(event.dataTransfer.files));
        }
      }}
    >
      {attachments.length > 0 && <ul aria-label="Image attachments" className="flex max-h-40 flex-wrap gap-2 overflow-y-auto">
        {attachments.map((image) => <li key={image.id} className="relative w-24 rounded-md border p-1.5">
          {image.url ? <a href={image.url} target="_blank" rel="noreferrer"><img src={image.url} alt={image.name} className="h-16 w-full rounded object-cover" /></a>
            : <div className="text-muted-foreground flex h-16 items-center justify-center text-xs">{image.error ? "Upload failed" : <LoaderCircle aria-label="Uploading image" className="size-4 motion-safe:animate-spin" />}</div>}
          <p className="text-muted-foreground mt-1 truncate text-[10px]" title={image.name}>{image.name}</p>
          {image.error && <p role="alert" className="text-destructive text-[10px]">{image.error}</p>}
          {onAttachmentRemove && <button type="button" aria-label={`Remove ${image.name}`} disabled={sending} className="bg-background focus-visible:ring-ring absolute top-0 right-0 rounded-full border p-0.5 outline-none focus-visible:ring-2" onClick={() => onAttachmentRemove(image.id)}><X aria-hidden="true" className="size-3" /></button>}
        </li>)}
      </ul>}
      {onFilesSelect && <input ref={fileInput} type="file" multiple accept="image/png,image/jpeg,image/webp,image/gif" aria-label="Choose images" className="hidden" disabled={disabled || sending} onChange={(event) => {
        const files = Array.from(event.target.files ?? []);
        event.target.value = "";
        if (files.length) onFilesSelect(files);
      }} /> }
      <textarea
        data-slot="chat-composer-input"
        rows={2}
        aria-label={inputLabel}
        placeholder={placeholder}
        {...inputProps}
        disabled={disabled}
        value={value}
        aria-describedby={[
          describedBy,
          helperId,
          failed && errorId,
          disabled && disabledId,
        ].filter(Boolean).join(" ")}
        className={cn(
          "text-foreground placeholder:text-muted-foreground focus-visible:ring-ring/50 field-sizing-content max-h-48 min-h-12 w-full resize-none rounded-md px-1 py-1 text-sm leading-6 outline-none focus-visible:ring-[3px] disabled:cursor-not-allowed disabled:opacity-50",
          inputClassName,
        )}
        onChange={(event) => {
          currentValue.current = event.target.value;
          setFailed(false);
          onValueChange(event.target.value);
        }}
        onKeyDown={(event) => {
          onKeyDown?.(event);
          if (
            event.defaultPrevented || !sendOnEnter || event.key !== "Enter" ||
            event.shiftKey || event.altKey || event.ctrlKey || event.metaKey ||
            event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229
          ) return;
          event.preventDefault();
          event.currentTarget.form?.requestSubmit();
        }}
      />
      <div className="flex min-w-0 items-end justify-between gap-3">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          {onFilesSelect && <Button type="button" variant="ghost" size="icon" className="size-7 shrink-0" aria-label="Attach images" disabled={disabled || sending} disabledReason={disabled ? reason! : "Wait for this message to finish sending."} onClick={() => fileInput.current?.click()}><Paperclip aria-hidden="true" className="size-4" /></Button>}
          {children}
          <p id={helperId} role="status" aria-live="polite" aria-atomic="true" data-slot="chat-composer-status" className={cn("flex items-center gap-1.5 text-xs leading-5", sending ? "text-foreground font-medium" : "text-muted-foreground")}>
            {sending && <LoaderCircle aria-hidden="true" className="size-3.5 shrink-0 motion-safe:animate-spin" />}
            {sending ? sendingLabel : sendOnEnter ? "Enter to send · Shift + Enter for a new line" : "Use the send button to send your message."}
          </p>
        </div>
        {isGenerating ? (
          <Button
            type="button"
            size="icon"
            variant="outline"
            aria-label={stopLabel}
            disabled={disabled || !onStop}
            disabledReason={disabled ? reason! : "This response cannot be stopped."}
            onClick={(event) => {
              // The host may synchronously replace this button with the submit action.
              event.preventDefault();
              onStop?.();
            }}
          >
            <Square className="size-3.5 fill-current" aria-hidden="true" />
          </Button>
        ) : (
          <Button
            type="submit"
            size="icon"
            aria-label={sendLabel}
            disabled={cannotSend}
            disabledReason={sendDisabledReason}
          >
            {sending ? (
              <LoaderCircle className="size-4 motion-safe:animate-spin" aria-hidden="true" />
            ) : (
              <ArrowUp className="size-4" aria-hidden="true" />
            )}
          </Button>
        )}
      </div>
      {disabled && <p id={disabledId} className="text-muted-foreground text-xs">{reason}</p>}
      {failed && <p id={errorId} role="alert" className="text-destructive text-sm">{errorMessage}</p>}
    </form>
  );
}

export { ChatComposer, type ChatComposerProps };
