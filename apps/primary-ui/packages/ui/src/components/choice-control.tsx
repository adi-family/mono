import * as React from "react";
import * as Popover from "@radix-ui/react-popover";
import { cva, type VariantProps } from "class-variance-authority";
import { Check, ChevronDown, LoaderCircle, Plus, X } from "lucide-react";

import { cn } from "../lib/cn";
import type { ChoiceOption } from "./choice-types";

const choiceVariants = cva(
  "border-input bg-background flex w-full min-w-0 items-center gap-1.5 rounded-md border px-2.5 text-sm transition-colors focus-within:border-ring focus-within:ring-ring/50 focus-within:ring-[3px] data-[disabled=true]:opacity-50 data-[invalid=true]:border-destructive",
  {
    variants: { size: { sm: "min-h-7 py-0.5", default: "min-h-8 py-1", lg: "min-h-9 py-1.5" } },
    defaultVariants: { size: "default" },
  },
);

export type ChoiceControlProps = Pick<
  React.ComponentProps<"input">,
  | "id" | "name" | "form" | "disabled" | "required" | "autoFocus" | "ref"
  | "aria-label" | "aria-labelledby" | "aria-describedby" | "aria-invalid"
  | "onFocus" | "onBlur" | "onKeyDown"
> & VariantProps<typeof choiceVariants> & {
  options?: readonly ChoiceOption[];
  /** The loader owns filtering; server-ranked results are not filtered again. */
  loadOptions?: (query: string, context: { signal: AbortSignal }) => Promise<readonly ChoiceOption[]>;
  debounceMs?: number;
  creatable?: boolean;
  /** Return the persisted option, or omit to create a local text value. */
  onCreate?: (input: string) => ChoiceOption | Promise<ChoiceOption>;
  placeholder?: string;
  clearable?: boolean;
  className?: string;
  emptyMessage?: string;
  loadingMessage?: string;
  errorMessage?: string;
  createErrorMessage?: string;
  createLabel?: (input: string) => string;
  clearLabel?: string;
  retryLabel?: string;
};

const EMPTY_OPTIONS: readonly ChoiceOption[] = [];
const normalize = (value: string) => value.trim().toLocaleLowerCase();
const matches = (option: ChoiceOption, query: string) =>
  [option.label, ...(option.keywords ?? [])].some((text) => normalize(text).includes(normalize(query)));

function uniqueOptions(options: readonly ChoiceOption[]) {
  return Array.from(new Map(options.map((option) => [option.value, option])).values());
}

type LoadResult = {
  query: string;
  loader: ChoiceControlProps["loadOptions"];
  status: "ready" | "loading" | "error";
  options: readonly ChoiceOption[];
};

/** Shared behavior for the public single and multiple searchable controls. */
export function ChoiceControl({
  multiple,
  selected,
  onSelectionChange,
  onReset,
  options = EMPTY_OPTIONS,
  loadOptions,
  debounceMs = 250,
  creatable = false,
  onCreate,
  placeholder = "Search options…",
  clearable = true,
  disabled = false,
  required = false,
  className,
  size,
  emptyMessage = "No results found.",
  loadingMessage = "Loading…",
  errorMessage = "Could not load options.",
  createErrorMessage = "Could not create this option. Try again.",
  createLabel = (input) => `Create “${input}”`,
  clearLabel = "Clear selection",
  retryLabel = "Retry",
  id,
  name,
  form,
  ref,
  onFocus,
  onBlur,
  onKeyDown,
  ...inputProps
}: ChoiceControlProps & {
  multiple: boolean;
  selected: readonly ChoiceOption[];
  onSelectionChange: (options: ChoiceOption[]) => void;
  onReset: () => void;
}) {
  const generatedId = React.useId();
  const inputId = id ?? generatedId;
  const listId = `${generatedId}-list`;
  const statusId = `${generatedId}-status`;
  const inputRef = React.useRef<HTMLInputElement>(null);
  const anchorRef = React.useRef<HTMLDivElement>(null);
  const listRef = React.useRef<HTMLDivElement>(null);
  const [open, setOpen] = React.useState(false);
  const [query, setQuery] = React.useState("");
  const [activeIndex, setActiveIndex] = React.useState(-1);
  const [created, setCreated] = React.useState<ChoiceOption[]>([]);
  const [creating, setCreating] = React.useState(false);
  const [createError, setCreateError] = React.useState(false);
  const [retry, setRetry] = React.useState(0);
  const [result, setResult] = React.useState<LoadResult | null>(null);
  const creationVersion = React.useRef(0);
  const selectedRef = React.useRef(selected);
  selectedRef.current = selected;
  const changeRef = React.useRef(onSelectionChange);
  changeRef.current = onSelectionChange;
  const resetRef = React.useRef(onReset);
  resetRef.current = onReset;
  const isOpen = open && !disabled;
  const search = query.trim();
  const currentResult = result?.query === search && result.loader === loadOptions ? result : null;
  const loading = Boolean(loadOptions && isOpen && (!currentResult || currentResult.status === "loading"));
  const loadError = Boolean(loadOptions && currentResult?.status === "error");
  const authoritativeOptions = loadOptions ? currentResult?.options ?? EMPTY_OPTIONS : options;
  const available = uniqueOptions([
    ...(loadOptions ? authoritativeOptions : authoritativeOptions.filter((option) => matches(option, search))),
    ...created.filter((option) =>
      !authoritativeOptions.some((current) => current.value === option.value) && matches(option, search),
    ),
  ]);
  const isOptionDisabled = (option: ChoiceOption) => Boolean(option.disabled
    || selected.some((item) => item.value === option.value && item.disabled)
    || authoritativeOptions.some((item) => item.value === option.value && item.disabled));
  const knownOptions = [...options, ...available, ...selected, ...created];
  const exactMatch = knownOptions.some((option) =>
    normalize(option.label) === normalize(search) || normalize(option.value) === normalize(search),
  );
  const canCreate = creatable && Boolean(search) && !exactMatch && !loading && !loadError;
  const visibleOptions = loading || loadError ? EMPTY_OPTIONS : available;
  const enabledIndices = visibleOptions.flatMap((option, index) => isOptionDisabled(option) ? [] : [index]);
  if (canCreate) enabledIndices.push(visibleOptions.length);
  const active = enabledIndices.includes(activeIndex) ? activeIndex : -1;
  const activeId = active >= 0 ? `${listId}-${active}` : undefined;

  const close = () => {
    setOpen(false);
    setQuery("");
    setActiveIndex(-1);
    setCreateError(false);
    setResult(null);
    creationVersion.current += 1;
    setCreating(false);
  };

  React.useEffect(() => {
    if (!loadOptions || !isOpen) return;
    const controller = new AbortController();
    setResult({ query: search, loader: loadOptions, status: "loading", options: EMPTY_OPTIONS });
    const timer = setTimeout(async () => {
      try {
        const loaded = await loadOptions(search, { signal: controller.signal });
        if (!controller.signal.aborted) {
          setResult({ query: search, loader: loadOptions, status: "ready", options: loaded });
        }
      } catch {
        if (!controller.signal.aborted) {
          setResult({ query: search, loader: loadOptions, status: "error", options: EMPTY_OPTIONS });
        }
      }
    }, Math.max(0, debounceMs));
    return () => { clearTimeout(timer); controller.abort(); };
  }, [loadOptions, search, isOpen, debounceMs, retry]);

  React.useEffect(() => {
    if (disabled) close();
    // Closing invalidates a pending create even if its promise ignores cancellation.
    return () => { creationVersion.current += 1; };
  }, [disabled]);

  React.useEffect(() => {
    inputRef.current?.setCustomValidity(required && selected.length === 0 ? "Please select an option." : "");
  }, [required, selected.length]);

  React.useEffect(() => {
    const ownerForm = inputRef.current?.form;
    const reset = (event: Event) => {
      // Respect reset cancellation by another listener.
      queueMicrotask(() => { if (!event.defaultPrevented) { close(); resetRef.current(); } });
    };
    ownerForm?.addEventListener("reset", reset);
    return () => ownerForm?.removeEventListener("reset", reset);
  }, [form]);

  React.useEffect(() => {
    if (activeId) document.getElementById(activeId)?.scrollIntoView({ block: "nearest" });
  }, [activeId]);

  function choose(option: ChoiceOption) {
    if (disabled || creating || isOptionDisabled(option)) return;
    if (multiple) {
      onSelectionChange(selected.some((item) => item.value === option.value)
        ? selected.filter((item) => item.value !== option.value)
        : [...selected, option]);
      setQuery("");
      setActiveIndex(-1);
    } else {
      onSelectionChange([option]);
      close();
    }
    inputRef.current?.focus();
  }

  async function createOption() {
    if (!canCreate || creating || disabled) return;
    const version = ++creationVersion.current;
    setCreating(true);
    setCreateError(false);
    try {
      const option = onCreate ? await onCreate(search) : { value: search, label: search };
      if (version !== creationVersion.current) return;
      if (!option || !option.value?.trim() || !option.label?.trim() || option.disabled) {
        throw new Error("onCreate must return an enabled option with a non-empty value and label.");
      }
      setCreated((previous) => uniqueOptions([...previous, option]));
      changeRef.current(multiple ? uniqueOptions([...selectedRef.current, option]) : [option]);
      setCreating(false);
      setQuery("");
      setActiveIndex(-1);
      if (!multiple) close();
      inputRef.current?.focus();
    } catch {
      if (version === creationVersion.current) { setCreateError(true); setCreating(false); }
    }
  }

  function handleKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    onKeyDown?.(event);
    if (event.defaultPrevented || event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      setOpen(true);
      const direction = event.key === "ArrowDown" ? 1 : -1;
      const index = enabledIndices.indexOf(active);
      const next = index < 0 ? (direction > 0 ? 0 : enabledIndices.length - 1)
        : (index + direction + enabledIndices.length) % enabledIndices.length;
      setActiveIndex(enabledIndices[next] ?? -1);
    } else if (event.key === "Enter" && isOpen) {
      event.preventDefault();
      if (creating) return;
      if (loadError) { setRetry((count) => count + 1); return; }
      if (active >= 0 && active < visibleOptions.length) choose(visibleOptions[active]);
      else if (active === visibleOptions.length && canCreate) void createOption();
    } else if (event.key === "Escape" && isOpen) {
      event.preventDefault();
      event.stopPropagation();
      close();
    } else if (event.key === "Tab") {
      close();
    } else if (event.key === "Backspace" && multiple && !query && !creating) {
      const lastEnabled = selected.map((option) => !isOptionDisabled(option)).lastIndexOf(true);
      if (lastEnabled >= 0) onSelectionChange(selected.filter((_, index) => index !== lastEnabled));
    }
  }

  const status = creating ? "Creating…" : loading ? loadingMessage : loadError ? errorMessage
    : createError ? createErrorMessage : isOpen && visibleOptions.length === 0 && !canCreate ? emptyMessage : "";
  const describedBy = [inputProps["aria-describedby"], status ? statusId : undefined].filter(Boolean).join(" ") || undefined;

  return (
    <Popover.Root open={isOpen} onOpenChange={(next) => { if (next && !disabled) setOpen(true); else close(); }}>
      <Popover.Anchor asChild>
        <div
          ref={anchorRef}
          data-slot={multiple ? "multi-select" : "autocomplete"}
          data-disabled={disabled}
          data-invalid={Boolean(inputProps["aria-invalid"] && inputProps["aria-invalid"] !== "false")}
          className={cn(choiceVariants({ size }), className)}
        >
          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1">
            {multiple && selected.map((option) => (
              <span key={option.value} data-slot="choice-tag" className="bg-secondary text-secondary-foreground inline-flex max-w-full items-center gap-1 rounded px-1.5 py-0.5 text-xs">
                <span className="truncate">{option.label}</span>
                <button
                  type="button"
                  aria-label={`Remove ${option.label}`}
                  disabled={disabled || creating || isOptionDisabled(option)}
                  className="hover:bg-accent focus-visible:ring-ring cursor-pointer rounded outline-none focus-visible:ring-2 disabled:cursor-not-allowed disabled:opacity-50"
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => { onSelectionChange(selected.filter((item) => item.value !== option.value)); inputRef.current?.focus(); }}
                ><X className="size-3" aria-hidden="true" /></button>
              </span>
            ))}
            <input
              {...inputProps}
              ref={(node) => {
                inputRef.current = node;
                if (typeof ref === "function") return ref(node);
                if (ref) ref.current = node;
              }}
              id={inputId}
              form={form}
              data-slot="choice-input"
              role="combobox"
              type="text"
              autoComplete="off"
              spellCheck={false}
              aria-autocomplete="list"
              aria-haspopup="listbox"
              aria-expanded={isOpen}
              aria-controls={isOpen ? listId : undefined}
              aria-activedescendant={isOpen ? activeId : undefined}
              aria-describedby={describedBy}
              aria-required={required || undefined}
              aria-busy={loading || creating || undefined}
              disabled={disabled}
              aria-readonly={creating || undefined}
              required={required && selected.length === 0}
              value={!multiple && !isOpen ? selected[0]?.label ?? "" : query}
              placeholder={multiple && selected.length ? "Add more…" : placeholder}
              className="placeholder:text-muted-foreground h-5 w-0 min-w-16 flex-1 bg-transparent outline-none disabled:cursor-not-allowed"
              onChange={(event) => {
                if (creating) return;
                setQuery(event.target.value); setOpen(true); setActiveIndex(-1); setCreateError(false);
              }}
              onFocus={onFocus}
              onClick={() => { if (!creating) setOpen(true); }}
              onBlur={(event) => {
                onBlur?.(event);
                if (!anchorRef.current?.contains(event.relatedTarget) && !listRef.current?.contains(event.relatedTarget)) close();
              }}
              onKeyDown={handleKeyDown}
            />
          </div>
          {clearable && selected.some((option) => !isOptionDisabled(option)) && (
            <button type="button" aria-label={clearLabel} disabled={disabled || creating}
              className="text-muted-foreground hover:text-foreground focus-visible:ring-ring shrink-0 cursor-pointer rounded outline-none focus-visible:ring-2 disabled:cursor-not-allowed"
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => { onSelectionChange(selected.filter(isOptionDisabled)); setQuery(""); inputRef.current?.focus(); }}
            ><X className="size-3.5" aria-hidden="true" /></button>
          )}
          <button type="button" tabIndex={-1} aria-label={isOpen ? "Close options" : "Show options"}
            disabled={disabled || creating}
            className="text-muted-foreground shrink-0 cursor-pointer disabled:cursor-not-allowed"
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => { if (isOpen) close(); else setOpen(true); inputRef.current?.focus(); }}
          >
            {loading || creating ? <LoaderCircle className="size-3.5 motion-safe:animate-spin" aria-hidden="true" /> : <ChevronDown className="size-3.5" aria-hidden="true" />}
          </button>
          {name && (selected.length ? selected : [{ value: "", label: "" }]).map((option, index) => (
            <input key={`${option.value}-${index}`} type="hidden" name={name} form={form} value={option.value} disabled={disabled || (multiple && !selected.length)} />
          ))}
        </div>
      </Popover.Anchor>
      <span id={statusId} role="status" aria-live="polite" className="sr-only">{status}</span>
      <Popover.Portal>
        <Popover.Content
          ref={listRef}
          data-slot="choice-content"
          role="presentation"
          align="start"
          sideOffset={4}
          collisionPadding={8}
          className="bg-popover text-popover-foreground z-50 w-[var(--radix-popover-trigger-width)] min-w-0 max-w-[var(--radix-popover-content-available-width)] rounded-md border p-1"
          onOpenAutoFocus={(event) => event.preventDefault()}
          onCloseAutoFocus={(event) => event.preventDefault()}
          onInteractOutside={(event) => { if (anchorRef.current?.contains(event.target as Node)) event.preventDefault(); }}
          onEscapeKeyDown={(event) => { event.preventDefault(); close(); inputRef.current?.focus(); }}
        >
          <div
            id={listId}
            role="listbox"
            aria-label={inputProps["aria-label"] ?? (inputProps["aria-labelledby"] ? undefined : "Options")}
            aria-labelledby={inputProps["aria-labelledby"]}
            aria-multiselectable={multiple || undefined}
            aria-busy={loading || creating || undefined}
            className="max-h-[min(16rem,var(--radix-popover-content-available-height))] overflow-y-auto overscroll-contain"
          >
            {visibleOptions.map((option, index) => (
              <div
                key={option.value}
                id={`${listId}-${index}`}
                data-slot="choice-option"
                role="option"
                aria-selected={selected.some((item) => item.value === option.value)}
                aria-disabled={isOptionDisabled(option) || creating || undefined}
                data-active={active === index}
                className="data-[active=true]:bg-accent data-[active=true]:text-accent-foreground flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm select-none aria-disabled:cursor-not-allowed aria-disabled:opacity-50"
                onPointerMove={() => { if (!isOptionDisabled(option) && !creating) setActiveIndex(index); }}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => choose(option)}
              >
                <span className="min-w-0 flex-1 break-words">{option.label}</span>
                {selected.some((item) => item.value === option.value) && <Check className="size-3.5 shrink-0" aria-hidden="true" />}
              </div>
            ))}
            {canCreate && (
              <div id={`${listId}-${visibleOptions.length}`} data-slot="choice-create" role="option" aria-selected={false} aria-disabled={creating || undefined}
                data-active={active === visibleOptions.length}
                className="data-[active=true]:bg-accent data-[active=true]:text-accent-foreground flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm select-none aria-disabled:cursor-not-allowed aria-disabled:opacity-50"
                onPointerMove={() => setActiveIndex(visibleOptions.length)}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => void createOption()}
              ><Plus className="size-3.5 shrink-0" aria-hidden="true" /><span className="break-words">{createLabel(search)}</span></div>
            )}
          </div>
          {status && <p aria-hidden="true" className={cn("px-2 py-2 text-sm", loadError || createError ? "text-destructive" : "text-muted-foreground")}>{status}</p>}
          {loadError && <button type="button" className="hover:bg-accent focus-visible:ring-ring m-1 cursor-pointer rounded border px-2 py-1 text-sm outline-none focus-visible:ring-2"
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => { setRetry((count) => count + 1); inputRef.current?.focus(); }}
          >{retryLabel}</button>}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
