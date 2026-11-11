import * as React from "react";
import { ArrowDown, ArrowUp, ChevronDown, Clock3 } from "lucide-react";
import { cva } from "class-variance-authority";

import { cn } from "../lib/cn";
import { Button } from "./button";

type ChatThreadDirection = "top-to-bottom" | "bottom-to-top";

type ChatThreadProps = React.ComponentProps<"div"> & {
  /** Pass messages oldest first; bottom-to-top displays the latest message at the top. */
  direction?: ChatThreadDirection;
  /** Follow new content while the reader remains close to the latest message. */
  autoScroll?: boolean;
  label?: string;
  scrollToLatestLabel?: string;
};

function ChatThread({
  direction = "top-to-bottom",
  autoScroll = true,
  label = "Conversation",
  scrollToLatestLabel = "Jump to latest",
  "aria-live": ariaLive = "polite",
  children,
  className,
  ...props
}: ChatThreadProps) {
  const viewportRef = React.useRef<HTMLDivElement>(null);
  const contentRef = React.useRef<HTMLDivElement>(null);
  const followingRef = React.useRef(true);
  const lastScrollTopRef = React.useRef(0);
  const previousDirectionRef = React.useRef(direction);
  const pendingDirectionFollowRef = React.useRef(false);
  const readingAnchorRef = React.useRef<{ element: Element; offset: number } | null>(null);
  const [awayFromLatest, setAwayFromLatest] = React.useState(false);
  const newestAtTop = direction === "bottom-to-top";
  const orderedChildren = React.Children.toArray(children);
  if (newestAtTop) orderedChildren.reverse();

  const rememberReadingAnchor = React.useCallback(() => {
    const viewport = viewportRef.current;
    const content = contentRef.current;
    if (!newestAtTop || !viewport || !content || viewport.clientHeight === 0) return;

    const viewportTop = viewport.getBoundingClientRect().top + viewport.clientTop;
    const viewportBottom = viewportTop + viewport.clientHeight;
    const visibleChild = Array.from(content.children).find((child) => {
      const rect = child.getBoundingClientRect();
      return rect.bottom > viewportTop && rect.top < viewportBottom;
    });
    readingAnchorRef.current = visibleChild
      ? { element: visibleChild, offset: visibleChild.getBoundingClientRect().top - viewportTop }
      : null;
  }, [newestAtTop]);

  const updatePosition = React.useCallback(() => {
    if (previousDirectionRef.current !== direction) {
      previousDirectionRef.current = direction;
      followingRef.current = true;
      pendingDirectionFollowRef.current = true;
      readingAnchorRef.current = null;
    }
    const viewport = viewportRef.current;
    // Hidden threads must retain the reader's position until they have a size again.
    if (!viewport || viewport.clientHeight === 0) return;

    if (pendingDirectionFollowRef.current || (autoScroll && followingRef.current)) {
      viewport.scrollTo({ top: newestAtTop ? 0 : viewport.scrollHeight, behavior: "instant" });
      pendingDirectionFollowRef.current = false;
    } else if (newestAtTop) {
      const anchor = readingAnchorRef.current;
      if (anchor && contentRef.current?.contains(anchor.element)) {
        // Newest messages are prepended in this direction. Keep the visible message at
        // its previous offset, including when a newer response grows while streaming.
        const viewportTop = viewport.getBoundingClientRect().top + viewport.clientTop;
        const offsetChange = anchor.element.getBoundingClientRect().top - viewportTop - anchor.offset;
        if (Math.abs(offsetChange) > 0.5) {
          viewport.scrollTo({ top: viewport.scrollTop + offsetChange, behavior: "instant" });
        }
      }
    }
    const nearLatest = newestAtTop
      ? viewport.scrollTop <= 64
      : viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight <= 64;
    lastScrollTopRef.current = viewport.scrollTop;
    followingRef.current = nearLatest;
    setAwayFromLatest(!nearLatest);
    rememberReadingAnchor();
  }, [autoScroll, direction, newestAtTop, rememberReadingAnchor]);

  React.useLayoutEffect(() => {
    updatePosition();
    const viewport = viewportRef.current;
    const content = contentRef.current;
    if (!viewport || !content || typeof ResizeObserver === "undefined") return;

    const observer = new ResizeObserver(updatePosition);
    observer.observe(viewport);
    observer.observe(content);
    return () => observer.disconnect();
  }, [updatePosition]);

  // Also handles direct child updates in environments without ResizeObserver.
  React.useLayoutEffect(() => { updatePosition(); }, [children, updatePosition]);

  function handleScroll() {
    const viewport = viewportRef.current;
    if (!viewport || viewport.clientHeight === 0) return;
    const nearLatest = newestAtTop
      ? viewport.scrollTop <= 64
      : viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight <= 64;
    const previousScrollTop = lastScrollTopRef.current;
    const movedAway = newestAtTop
      ? viewport.scrollTop > previousScrollTop
      : viewport.scrollTop < previousScrollTop;
    if (nearLatest) {
      followingRef.current = true;
    } else if (!autoScroll || movedAway) {
      followingRef.current = false;
    }
    // A scroll event queued by our previous positioning can arrive after content grows.
    // Only movement away from the latest message should cancel automatic following.
    lastScrollTopRef.current = viewport.scrollTop;
    setAwayFromLatest(!followingRef.current);
    // A queued scroll event without movement must not replace an anchor captured
    // before a resize that is still waiting for its ResizeObserver callback.
    if (viewport.scrollTop !== previousScrollTop) rememberReadingAnchor();
  }

  function jumpToLatest() {
    const viewport = viewportRef.current;
    if (!viewport) return;
    followingRef.current = true;
    // Immediate positioning avoids motion even when the host uses scroll-smooth.
    viewport.scrollTo({ top: newestAtTop ? 0 : viewport.scrollHeight, behavior: "instant" });
    lastScrollTopRef.current = viewport.scrollTop;
    setAwayFromLatest(false);
    rememberReadingAnchor();
    viewport.focus({ preventScroll: true });
  }

  return (
    <div
      data-slot="chat-thread"
      data-direction={direction}
      className={cn("relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden", className)}
      {...props}
    >
      <div
        ref={viewportRef}
        data-slot="chat-thread-viewport"
        role="log"
        aria-label={label}
        aria-live={ariaLive}
        aria-relevant="additions text"
        aria-atomic="false"
        tabIndex={0}
        onScroll={handleScroll}
        className={cn(
          "focus-visible:ring-ring/50 min-h-0 min-w-0 flex-1 overflow-y-auto overscroll-contain outline-none focus-visible:ring-2 focus-visible:ring-inset",
          newestAtTop && "[overflow-anchor:none]",
        )}
      >
        <div ref={contentRef} data-slot="chat-thread-content" className="flex min-w-0 flex-col gap-6 p-3 sm:p-4">
          {orderedChildren}
        </div>
      </div>
      {awayFromLatest && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          data-slot="chat-thread-jump"
          className={cn("absolute right-4 shadow-sm", newestAtTop ? "top-3" : "bottom-3")}
          onClick={jumpToLatest}
        >
          {newestAtTop
            ? <ArrowUp className="size-3.5" aria-hidden="true" />
            : <ArrowDown className="size-3.5" aria-hidden="true" />}
          {scrollToLatestLabel}
        </Button>
      )}
    </div>
  );
}

type ChatMessageFrom = "user" | "assistant" | "system";

const chatMessageVariants = cva("group/chat-message flex min-w-0 gap-3 text-sm", {
  variants: {
    from: {
      user: "flex-row-reverse self-end",
      assistant: "self-start",
      system: "w-full justify-center self-center text-muted-foreground",
    },
  },
});

type ChatMessageProps = React.ComponentProps<"article"> & { from: ChatMessageFrom };

function ChatMessage({ from, className, ...props }: ChatMessageProps) {
  return (
    <article
      data-slot="chat-message"
      data-from={from}
      className={cn(chatMessageVariants({ from }), "max-w-[90%]", className)}
      {...props}
    />
  );
}

function ChatMessageAvatar({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="chat-message-avatar"
      className={cn(
        "bg-muted text-muted-foreground flex size-8 shrink-0 items-center justify-center overflow-hidden rounded-full text-xs font-medium [&_svg]:size-4",
        className,
      )}
      {...props}
    />
  );
}

function ChatMessageContent({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="chat-message-content"
      className={cn(
        "min-w-0 leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere] group-data-[from=user]/chat-message:rounded-2xl group-data-[from=user]/chat-message:bg-muted group-data-[from=user]/chat-message:px-3 group-data-[from=user]/chat-message:py-3 group-data-[from=user]/chat-message:text-foreground sm:group-data-[from=user]/chat-message:px-4 group-data-[from=system]/chat-message:py-1 group-data-[from=system]/chat-message:text-center group-data-[from=system]/chat-message:text-xs",
        className,
      )}
      {...props}
    />
  );
}

function ChatMessageMeta({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="chat-message-meta"
      className={cn("text-muted-foreground mb-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs group-data-[from=user]/chat-message:justify-end", className)}
      {...props}
    />
  );
}

function ChatMessageActions({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="chat-message-actions"
      className={cn("mt-1.5 flex flex-wrap items-center gap-1 group-data-[from=user]/chat-message:justify-end", className)}
      {...props}
    />
  );
}

type ChatTypingIndicatorProps = React.ComponentProps<"div"> & { label?: string };

function ChatTypingIndicator({ label = "Assistant is typing", className, ...props }: ChatTypingIndicatorProps) {
  return (
    <div
      data-slot="chat-typing-indicator"
      role="status"
      className={cn("text-muted-foreground inline-flex items-center gap-1 py-1", className)}
      {...props}
    >
      <span className="sr-only">{label}</span>
      <span aria-hidden="true" className="bg-current size-1.5 rounded-full motion-safe:animate-pulse" />
      <span aria-hidden="true" className="bg-current size-1.5 rounded-full motion-safe:animate-pulse [animation-delay:150ms]" />
      <span aria-hidden="true" className="bg-current size-1.5 rounded-full motion-safe:animate-pulse [animation-delay:300ms]" />
    </div>
  );
}

const rowCancelClass = "h-6 shrink-0 px-2 text-xs";

/** Enough of a row's text to tell its Cancel button from its neighbours'. */
function excerpt(text: string): string {
  const flat = text.trim().replace(/\s+/g, " ");
  return flat.length > 60 ? `${flat.slice(0, 60)}…` : flat;
}

type ChatWaitingStatusProps = Omit<React.ComponentProps<"details">, "children"> & {
  items: readonly { id: string; label: string }[];
  /** Adds a Cancel button to every task in the expanded list. */
  onCancel?: (id: string) => void;
  /** Tasks whose cancellation is still in flight. */
  cancellingIds?: readonly string[];
};

function ChatWaitingStatus({ items, onCancel, cancellingIds = [], className, ...props }: ChatWaitingStatusProps) {
  if (!items.length) return null;
  return (
    <details data-slot="chat-waiting-status" aria-label="Waiting status" className={cn("group/waiting text-muted-foreground min-w-0 rounded-md bg-foreground/5 text-xs", className)} {...props}>
      <summary className="focus-visible:ring-ring flex min-w-0 cursor-pointer list-none items-center gap-2 rounded-md px-3 py-2 outline-none focus-visible:ring-2 [&::-webkit-details-marker]:hidden">
        <Clock3 aria-hidden="true" className="size-3.5 shrink-0" />
        <span className="shrink-0 font-medium">Waiting on {items.length} {items.length === 1 ? "task" : "tasks"}</span>
        <span className="min-w-0 flex-1 truncate">{items[0].label}</span>
        <ChevronDown aria-hidden="true" className="size-3 shrink-0 group-open/waiting:rotate-180" />
      </summary>
      <ul className={cn("max-h-32 space-y-1 overflow-y-auto pb-2 pl-8.5 leading-5 [overflow-wrap:anywhere]", onCancel ? "pr-1" : "pr-3")}>
        {items.map((item) => onCancel ? (
          <li key={item.id} className="flex min-w-0 items-start gap-2">
            <span className="min-w-0 flex-1 py-0.5">{item.label}</span>
            <Button
              data-slot="chat-waiting-cancel"
              type="button"
              size="sm"
              variant="ghost"
              className={rowCancelClass}
              aria-label={`Cancel waiting: ${excerpt(item.label)}`}
              disabled={cancellingIds.includes(item.id)}
              disabledReason="This task is being cancelled."
              onClick={() => onCancel(item.id)}
            >
              Cancel
            </Button>
          </li>
        ) : <li key={item.id}>{item.label}</li>)}
      </ul>
    </details>
  );
}

type ChatQueuedMessageProps = React.ComponentProps<"details"> & {
  text: string;
  label?: string;
  /** Adds a Cancel button beside the row. */
  onCancel?: () => void;
  /** The cancellation is still in flight. */
  cancelling?: boolean;
};

function ChatQueuedMessage({ text, label = "Queued", onCancel, cancelling = false, children, className, ...props }: ChatQueuedMessageProps) {
  return (
    // The button sits beside the details, never inside its summary: a summary is itself a control.
    <div data-slot="chat-queued-row" className={cn("text-muted-foreground flex min-w-0 items-start rounded-md bg-foreground/5 text-xs", className)}>
      <details data-slot="chat-queued-message" className="min-w-0 flex-1" {...props}>
        <summary className="focus-visible:ring-ring flex min-w-0 cursor-pointer list-none items-center gap-2 rounded-md px-3 py-2 outline-none focus-visible:ring-2 [&::-webkit-details-marker]:hidden">
          <Clock3 aria-hidden="true" className="size-3.5 shrink-0" />
          <span className="shrink-0 font-medium">{label}</span>
          <span className="min-w-0 flex-1 truncate">{text || "Image attachment"}</span>
        </summary>
        <div className="max-h-48 overflow-y-auto px-3 pb-3 whitespace-pre-wrap [overflow-wrap:anywhere]">{text}{children}</div>
      </details>
      {onCancel && (
        <Button
          data-slot="chat-queued-cancel"
          type="button"
          size="sm"
          variant="ghost"
          className={cn(rowCancelClass, "m-1")}
          aria-label={`Cancel queued message: ${excerpt(text) || "Image attachment"}`}
          disabled={cancelling}
          disabledReason="This message is being cancelled."
          onClick={onCancel}
        >
          Cancel
        </Button>
      )}
    </div>
  );
}

export {
  ChatWaitingStatus,
  type ChatWaitingStatusProps,
  ChatQueuedMessage,
  type ChatQueuedMessageProps,
  ChatThread,
  ChatMessage,
  ChatMessageAvatar,
  ChatMessageContent,
  ChatMessageMeta,
  ChatMessageActions,
  ChatTypingIndicator,
  type ChatThreadProps,
  type ChatThreadDirection,
  type ChatMessageProps,
  type ChatMessageFrom,
  type ChatTypingIndicatorProps,
};
