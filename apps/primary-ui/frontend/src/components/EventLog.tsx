import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type RefObject } from "react";
import { createPortal } from "react-dom";
import { ScrollText, X } from "lucide-react";

import { Button, cn } from "@design-system/ui";

import { conversationAge } from "../lib/app-chat-sidebar";
import { clearEvents, markEventsSeen, setEventLogOpen, unreadEvents, useEventLog, useEventLogOpen, type AppEvent } from "../lib/event-log";
import { useIsMobile } from "../lib/use-is-mobile";

const panelWidth = 352;

function EventRow({ event, isNew, now }: { event: AppEvent; isNew: boolean; now: number }) {
  const when = new Date(event.lastSeen);
  return (
    <li data-slot="event-log-item" data-level={event.level} className="flex gap-2.5 rounded-md px-2 py-1.5">
      <span aria-hidden="true" className={cn("mt-[7px] size-1.5 shrink-0 rounded-full", event.level === "error" ? "bg-destructive" : "bg-foreground/40")} />
      <div className="min-w-0 flex-1">
        <p className="text-xs leading-5 [overflow-wrap:anywhere]">
          <span className="sr-only">{event.level === "error" ? "Error: " : "Note: "}</span>
          {event.message}
        </p>
        <p className="text-muted-foreground flex min-w-0 flex-wrap items-center gap-x-1.5 text-[11px] leading-4">
          <span className="truncate">{event.source ?? "This machine"} · {event.area}</span>
          <time dateTime={when.toISOString()} title={`Last seen: ${when.toLocaleString()}`}>{conversationAge(event.lastSeen, now) ?? ""}</time>
          {event.count > 1 && <span title={`Repeated ${event.count} times since ${new Date(event.firstSeen).toLocaleString()}`}>×{event.count}</span>}
          {isNew && <span data-slot="event-log-new" className="text-foreground">New</span>}
        </p>
      </div>
    </li>
  );
}

function EventLogPanel({ anchor, seenBefore, onClose }: { anchor: RefObject<HTMLButtonElement | null>; seenBefore: number; onClose: (restoreFocus?: boolean) => void }) {
  const { events } = useEventLog();
  const isMobile = useIsMobile();
  const element = useRef<HTMLDivElement>(null);
  const [style, setStyle] = useState<CSSProperties>({});
  const [now, setNow] = useState(Date.now);

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  useLayoutEffect(() => {
    const bounds = anchor.current?.getBoundingClientRect();
    if (!bounds) return;
    // Desktop: beside the rail, bottom edge level with the button. Phone: a sheet above the tab bar.
    setStyle(isMobile
      ? { left: 8, right: 8, bottom: window.innerHeight - bounds.top + 8, maxHeight: "60svh" }
      : { left: Math.min(bounds.right + 8, window.innerWidth - panelWidth - 8), bottom: Math.max(8, window.innerHeight - bounds.bottom), width: panelWidth, maxHeight: "min(70svh, calc(100svh - 1rem))" });
    element.current?.focus({ preventScroll: true });
  }, [anchor, isMobile]);

  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !element.current?.contains(event.target) && !anchor.current?.contains(event.target)) onClose(false);
    };
    // Escape is read at the document so it still closes the panel when focus has drifted, for
    // instance after the Clear button removed itself. A menu that handled it first is left alone.
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) onClose();
    };
    const reposition = () => onClose(false);
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    window.addEventListener("resize", reposition);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape);
      window.removeEventListener("resize", reposition);
    };
  }, [anchor, onClose]);

  const clear = () => {
    clearEvents();
    element.current?.focus({ preventScroll: true });
  };

  return createPortal(
    <div
      ref={element}
      data-slot="event-log"
      role="dialog"
      aria-label="Event log"
      tabIndex={-1}
      className="bg-popover text-popover-foreground border-border fixed z-50 flex flex-col overflow-hidden rounded-md border shadow-md outline-none"
      style={style}
    >
      <div className="flex shrink-0 items-center gap-1 border-b px-3 py-2">
        <h2 className="mr-auto text-sm font-semibold">Event log</h2>
        {events.length > 0 && <Button type="button" size="sm" variant="ghost" className="h-7 px-2 text-xs" onClick={clear}>Clear</Button>}
        <Button type="button" size="icon" variant="ghost" className="size-7" aria-label="Close event log" onClick={() => onClose()}>
          <X aria-hidden="true" className="size-3.5" />
        </Button>
      </div>
      {events.length === 0 ? (
        <div className="px-4 py-6 text-center">
          <p className="text-sm font-medium">Nothing to report</p>
          <p className="text-muted-foreground mt-1 text-xs leading-relaxed">Background failures, such as a machine that stops answering, are collected here instead of interrupting the page.</p>
        </div>
      ) : (
        <ul aria-label="Events" className="min-h-0 flex-1 overflow-y-auto overscroll-contain p-1">
          {events.map((event) => <EventRow key={event.id} event={event} isNew={event.lastSeen > seenBefore} now={now} />)}
        </ul>
      )}
    </div>,
    document.body,
  );
}

/**
 * The rail button that opens the log. `tabClass` is the rail's own tab styling, so the button
 * sits with the other tabs on every width; the dot marks unread events (red when any is an error).
 */
export function EventLogButton({ tabClass }: { tabClass: (active: boolean) => string }) {
  const log = useEventLog();
  const open = useEventLogOpen();
  const anchor = useRef<HTMLButtonElement>(null);
  const [seenBefore, setSeenBefore] = useState(log.seenAt);
  const unread = unreadEvents(log);
  const unreadErrors = unread.some((event) => event.level === "error");
  const label = unread.length ? `Event log, ${unread.length} new` : "Event log";

  // Opening marks the log read, but the panel still points out what arrived since the last look;
  // closing marks read again, for anything that arrived while it was open.
  const seenAt = log.seenAt;
  useEffect(() => {
    if (!open) return;
    setSeenBefore(seenAt);
    markEventsSeen();
    return () => markEventsSeen();
    // Depends on `open` alone: the snapshot is taken when the panel opens, not on every change.
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  const close = (restoreFocus = true) => {
    if (restoreFocus) anchor.current?.focus();
    setEventLogOpen(false);
  };

  return (
    <>
      <button
        ref={anchor}
        type="button"
        data-slot="event-log-button"
        data-unread={unread.length || undefined}
        aria-label={label}
        title={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        className={cn(tabClass(open), "relative")}
        onClick={() => setEventLogOpen(!open)}
      >
        <ScrollText aria-hidden="true" className="size-6 md:size-4" />
        {unread.length > 0 && (
          <span
            aria-hidden="true"
            data-slot="event-log-unread"
            className={cn(
              "border-background absolute top-1 right-[calc(50%-14px)] size-2.5 rounded-full border-2 md:top-1.5 md:right-1.5 md:size-2 md:border",
              unreadErrors ? "bg-destructive" : "bg-foreground/60",
            )}
          />
        )}
        <span className="md:sr-only">Events</span>
      </button>
      {open && <EventLogPanel anchor={anchor} seenBefore={seenBefore} onClose={close} />}
    </>
  );
}
