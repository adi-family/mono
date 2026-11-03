import * as React from "react";
import {
  Ban,
  Bot,
  Check,
  ChevronDown,
  CircleAlert,
  Clock3,
  FileCode2,
  FilePenLine,
  GitBranch,
  LoaderCircle,
  SquareTerminal,
  Wrench,
} from "lucide-react";
import { cva } from "class-variance-authority";

import { cn } from "../lib/cn";

type ChatToolCallKind = "terminal" | "git" | "edit" | "agent" | "tool";
type ChatToolCallStatus = "pending" | "running" | "completed" | "error" | "cancelled";

type ChatToolCallProps = React.ComponentProps<"div"> & {
  title: string;
  kind?: ChatToolCallKind;
  status?: ChatToolCallStatus;
  description?: string;
  meta?: React.ReactNode;
  defaultOpen?: boolean;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  statusLabels?: Partial<Record<ChatToolCallStatus, string>>;
  /** Group summaries stay neutral; only their error count is highlighted. */
  group?: { errorCount?: number };
  /** A destination for the activity's own conversation or related page. */
  href?: string;
  linkProps?: Omit<React.ComponentProps<"a">, "href" | "children">;
};

const kindIcons = { terminal: SquareTerminal, git: GitBranch, edit: FilePenLine, agent: Bot, tool: Wrench };
const statusIcons = {
  pending: Clock3,
  running: LoaderCircle,
  completed: Check,
  error: CircleAlert,
  cancelled: Ban,
};
const defaultStatusLabels: Record<ChatToolCallStatus, string> = {
  pending: "Pending",
  running: "Running",
  completed: "Completed",
  error: "Failed",
  cancelled: "Cancelled",
};
const summaryVariants = cva("flex min-w-0 items-center gap-1.5 text-xs leading-5", {
  variants: {
    status: {
      pending: "text-muted-foreground",
      running: "text-muted-foreground",
      completed: "text-muted-foreground",
      error: "text-destructive",
      cancelled: "text-muted-foreground",
    },
  },
});

/** A tool execution summary; the host owns execution, output, and cancellation. */
function ChatToolCall({
  title,
  kind = "tool",
  status = "completed",
  description,
  meta,
  defaultOpen = false,
  open,
  onOpenChange,
  statusLabels,
  group,
  href,
  linkProps,
  className,
  children,
  ...props
}: ChatToolCallProps) {
  const contentId = React.useId();
  const [internalOpen, setInternalOpen] = React.useState(defaultOpen);
  const expanded = open ?? internalOpen;
  const hasOutput = React.Children.toArray(children).length > 0;
  const hasContent = hasOutput || Boolean(description) || meta != null;
  const Icon = status === "completed" || (group && status === "error") ? kindIcons[kind] : statusIcons[status];
  const summaryStatus = group && status === "error" ? "completed" : status;
  const hoverColor = summaryStatus === "error" ? "hover:text-destructive" : "hover:text-foreground";
  const errorCount = group?.errorCount;
  const errorSummary = errorCount !== undefined
    ? errorCount > 0 ? `${errorCount} ${errorCount === 1 ? "error" : "errors"}` : null
    : group && status === "error" ? "Error" : null;
  const summary = (
    <>
      <Icon className={cn("size-3.5 shrink-0", status === "running" && "motion-safe:animate-spin")} aria-hidden="true" />
      <span
        data-slot="chat-tool-call-title"
        title={title}
        className={cn("min-w-0 truncate", (kind === "terminal" || kind === "git") && "font-mono")}
      >
        {title}
      </span>
      {errorSummary && <span data-slot="chat-tool-call-error-count" className="text-destructive shrink-0">{errorSummary}</span>}
      <span data-slot="chat-tool-call-status" className="sr-only">
        {statusLabels?.[status] ?? defaultStatusLabels[status]}
      </span>
      {hasContent && href === undefined && <ChevronDown aria-hidden="true" className={cn("size-3 shrink-0 opacity-40 group-hover/tool-call:opacity-100 group-focus-visible/tool-call:opacity-100", expanded && "rotate-180 opacity-100")} />}
    </>
  );

  function toggleOpen() {
    const nextOpen = !expanded;
    if (open === undefined) setInternalOpen(nextOpen);
    onOpenChange?.(nextOpen);
  }

  return (
    <div
      data-slot="chat-tool-call"
      data-kind={kind}
      data-status={status}
      data-grouped={group ? "true" : undefined}
      data-state={hasContent && expanded ? "open" : "closed"}
      className={cn("min-w-0 text-sm", className)}
      {...props}
    >
      <div data-slot="chat-tool-call-header" className="flex min-w-0 items-center gap-1.5">
        {href !== undefined ? (
          <>
            <a
              {...linkProps}
              href={href}
              data-slot="chat-tool-call-link"
              className={cn(summaryVariants({ status: summaryStatus }), hoverColor, "focus-visible:ring-ring/50 rounded-sm py-1 outline-none focus-visible:ring-2", linkProps?.className)}
            >
              {summary}
            </a>
            {hasContent && (
              <button
                type="button"
                data-slot="chat-tool-call-trigger"
                aria-label={`Details for ${title}`}
                aria-expanded={expanded}
                aria-controls={contentId}
                onClick={toggleOpen}
                className="group/tool-call text-muted-foreground hover:text-foreground focus-visible:ring-ring/50 inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-sm outline-none focus-visible:ring-2"
              >
                <ChevronDown aria-hidden="true" className={cn("size-3 opacity-40 group-hover/tool-call:opacity-100 group-focus-visible/tool-call:opacity-100", expanded && "rotate-180 opacity-100")} />
              </button>
            )}
          </>
        ) : hasContent ? (
          <button
            type="button"
            data-slot="chat-tool-call-trigger"
            aria-expanded={expanded}
            aria-controls={contentId}
            onClick={toggleOpen}
            className={cn(summaryVariants({ status: summaryStatus }), hoverColor, "group/tool-call focus-visible:ring-ring/50 w-full cursor-pointer rounded-sm py-1 text-left outline-none focus-visible:ring-2")}
          >
            {summary}
          </button>
        ) : (
          <div className={cn(summaryVariants({ status: summaryStatus }), "py-1")}>{summary}</div>
        )}
      </div>
      {hasContent && (
        <div id={contentId} data-slot="chat-tool-call-content" hidden={!expanded} className="ml-5 min-w-0 pb-2">
          {(description || meta != null) && (
            <div className="text-muted-foreground flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 py-1 text-[11px] [overflow-wrap:anywhere]">
              {description && <span>{description}</span>}
              {meta != null && <span data-slot="chat-tool-call-meta">{meta}</span>}
            </div>
          )}
          {hasOutput && <div className="overflow-hidden rounded-md border">{children}</div>}
        </div>
      )}
    </div>
  );
}

type ChatToolOutputProps = Omit<React.ComponentProps<"pre">, "children"> & {
  children: string;
  label?: string;
};

function ChatToolOutput({ label = "Tool output", className, children, tabIndex = 0, ...props }: ChatToolOutputProps) {
  return (
    <pre
      data-slot="chat-tool-output"
      role="region"
      aria-label={label}
      tabIndex={tabIndex}
      className={cn("bg-muted/40 text-foreground focus-visible:ring-ring/50 m-0 max-h-64 min-w-0 overflow-auto overscroll-contain p-3 font-mono text-xs leading-5 whitespace-pre outline-none focus-visible:ring-2 focus-visible:ring-inset", className)}
      {...props}
    >
      <code>{children}</code>
    </pre>
  );
}

type ChatToolDiffLine = {
  type: "context" | "addition" | "deletion";
  content: string;
  oldLineNumber?: number;
  newLineNumber?: number;
};

type ChatToolDiffProps = React.ComponentProps<"div"> & {
  file: string;
  lines: readonly ChatToolDiffLine[];
};

function ChatToolDiff({ file, lines, className, ...props }: ChatToolDiffProps) {
  const additions = lines.filter((line) => line.type === "addition").length;
  const deletions = lines.filter((line) => line.type === "deletion").length;
  const lineNumberWidth = lines.reduce((width, line) => Math.max(
    width,
    String(line.oldLineNumber ?? "").length,
    String(line.newLineNumber ?? "").length,
  ), 3);

  return (
    <div data-slot="chat-tool-diff" className={cn("min-w-0 text-xs", className)} {...props}>
      <div data-slot="chat-tool-diff-header" className="bg-muted/40 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 border-b px-3 py-2">
        <span className="flex min-w-0 flex-1 items-center gap-2 font-mono">
          <FileCode2 className="text-muted-foreground size-3.5 shrink-0" aria-hidden="true" />
          <span className="[overflow-wrap:anywhere]">{file}</span>
        </span>
        <span className="flex shrink-0 items-center gap-2 font-mono" aria-label={`${additions} additions, ${deletions} deletions`}>
          <span aria-hidden="true" className="text-emerald-700 dark:text-emerald-400">+{additions}</span>
          <span aria-hidden="true" className="text-destructive">−{deletions}</span>
        </span>
      </div>
      <div
        data-slot="chat-tool-diff-viewport"
        role="region"
        aria-label={`Changes to ${file}`}
        tabIndex={0}
        className="focus-visible:ring-ring/50 max-h-72 overflow-auto overscroll-contain py-1 font-mono leading-5 outline-none focus-visible:ring-2 focus-visible:ring-inset"
      >
        <div className="w-max min-w-full">
          {lines.map((line, index) => (
            <div
              key={index}
              data-slot="chat-tool-diff-line"
              data-type={line.type}
              className={cn(
                "flex whitespace-pre",
                line.type === "addition" && "bg-emerald-500/10 text-emerald-800 dark:text-emerald-300",
                line.type === "deletion" && "bg-destructive/10 text-destructive",
              )}
            >
              <span aria-hidden="true" style={{ width: `${lineNumberWidth + 2}ch` }} className="text-muted-foreground shrink-0 select-none pr-2 text-right tabular-nums">{line.oldLineNumber}</span>
              <span aria-hidden="true" style={{ width: `${lineNumberWidth + 2}ch` }} className="text-muted-foreground shrink-0 select-none pr-2 text-right tabular-nums">{line.newLineNumber}</span>
              <span className="w-5 shrink-0 select-none text-center">{line.type === "addition" ? "+" : line.type === "deletion" ? "−" : " "}</span>
              <code className="block pr-3">{line.content || " "}</code>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

export {
  ChatToolCall,
  ChatToolOutput,
  ChatToolDiff,
  type ChatToolCallKind,
  type ChatToolCallStatus,
  type ChatToolCallProps,
  type ChatToolOutputProps,
  type ChatToolDiffLine,
  type ChatToolDiffProps,
};
