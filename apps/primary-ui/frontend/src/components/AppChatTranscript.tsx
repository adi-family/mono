import { useEffect, useRef, useState, type ReactNode } from "react";
import { Pencil } from "lucide-react";
import {
  Button,
  ChatMessage,
  ChatMessageContent,
  ChatMessageMeta,
  ChatToolCall,
  ChatToolCode,
  ChatToolOutput,
  ChatTypingIndicator,
} from "@design-system/ui";
import {
  attachmentUrl,
  getSteps,
  sentWithPrevious,
  type AgentPeek,
  type AgentStep,
  type AgentTurn,
  type Conversation,
  type ConversationRef,
} from "../lib/app-chat";
import {
  activityStatus,
  agentLaunches,
  conversationHref,
  toolArguments,
  toolCode,
  toolDisplayName,
  toolSummary,
  transcriptParts,
} from "../lib/app-chat-transcript";
import { groupErrorCount } from "../lib/app-chat-activity";
import { ChatMarkdown } from "./ChatMarkdown";
import { conversationKey, sameConversation } from "../lib/app-chat-sources";

export interface AppChatTranscriptProps {
  conversation: ConversationRef;
  snapshot: AgentPeek;
  conversations: Conversation[];
  onOpenConversation: (ref: ConversationRef) => void;
  /** Offers an edit button on sent messages; the handler resets the chat to that message. */
  onEditMessage?: (turn: AgentTurn) => void;
  /** Set while a reset is in flight, when no other message can be edited. */
  editDisabledReason?: string;
}

type ActivityProps = Pick<
  AppChatTranscriptProps,
  "conversation" | "conversations" | "onOpenConversation"
> & { turn: number; stats?: AgentPeek["stats"] };
type ToolStep = Extract<AgentStep, { kind: "tool" }>;
type FoldedStep = Extract<AgentStep, { kind: "calls" }>;

function ToolArguments({ name: tool, input }: { name: string; input: string }) {
  const all = toolArguments(input);
  // A tool that runs a program shows it as highlighted code; what remains are its options.
  const program = toolCode(tool);
  const code = program ? all.find((arg) => arg.name === program.argument) : undefined;
  const args = code ? all.filter((arg) => arg !== code) : all;
  if (!all.length) return null;
  return (
    <>
      {program && code && <ChatToolCode language={program.language}>{code.value}</ChatToolCode>}
      {args.length > 0 && (
        <dl className={`space-y-2 px-3 py-2 text-xs${code ? " border-t" : ""}`}>
          {args.map(({ name, value }) => {
            const lengthy = value.length > 400 || value.split("\n").length > 12;
            return (
              <div key={name}>
                <dt className="text-muted-foreground mb-0.5">{name}</dt>
                <dd
                  tabIndex={lengthy ? 0 : undefined}
                  role={lengthy ? "region" : undefined}
                  aria-label={lengthy ? `${name} argument` : undefined}
                  className="focus-visible:ring-ring/50 m-0 max-h-64 overflow-auto overscroll-x-contain rounded-sm font-mono whitespace-pre-wrap outline-none focus-visible:ring-2 focus-visible:ring-inset wrap-anywhere"
                >
                  {value}
                </dd>
              </div>
            );
          })}
        </dl>
      )}
    </>
  );
}

function ToolActivity({
  step,
  conversation,
  conversations,
  onOpenConversation,
}: ActivityProps & { step: ToolStep }) {
  const children = agentLaunches(step).map((ref) => ({
    ...ref,
    ...(conversation.source ? { source: conversation.source } : {}),
  }));
  const details = (
    <>
      <ToolArguments name={step.name} input={step.input} />
      {step.output && <ChatToolOutput>{step.output}</ChatToolOutput>}
      {!step.output && step.status === "running" && (
        <p className="text-muted-foreground px-3 py-2 text-xs">Waiting for output…</p>
      )}
    </>
  );
  if (children.length)
    return children.map((ref) => {
      const child = conversations.find((item) => sameConversation(item, ref));
      return (
        <ChatToolCall
          key={conversationKey(ref)}
          kind="agent"
          title={[ref.agent, child?.title].filter(Boolean).join(" · ")}
          status={child?.running ? "running" : child?.state === "waiting" ? "pending" : "completed"}
          statusLabels={{ completed: child ? "Conversation available" : "Started" }}
          href={conversationHref(ref)}
          linkProps={{
            onClick: (event) => {
              if (
                event.defaultPrevented ||
                event.button !== 0 ||
                event.metaKey ||
                event.ctrlKey ||
                event.shiftKey ||
                event.altKey
              )
                return;
              event.preventDefault();
              onOpenConversation(ref);
            },
          }}
        >
          {details}
        </ChatToolCall>
      );
    });
  const summary = toolSummary(step.name, step.input);
  return (
    <ChatToolCall
      {...summary}
      status={activityStatus(step.status)}
      statusLabels={{ cancelled: "No result returned" }}
    >
      {details}
    </ChatToolCall>
  );
}

/** One read of a run's calls: the run they belong to, and the request that answered them. */
type LoadedSteps = { run: string; key: string; steps: AgentStep[] };

function FoldedActivity({ step, ...props }: ActivityProps & { step: FoldedStep }) {
  const [open, setOpen] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [loaded, setLoaded] = useState<LoadedSteps | null>(null);
  const [error, setError] = useState<string | null>(null);
  const latest = useRef<LoadedSteps | null>(null);
  const failed = useRef<{ key: string; attempt: number } | null>(null);
  const { agent, runId, source } = props.conversation;
  const { turn } = props;
  const { from, to, count, status } = step;
  // Where a run starts is what it is; its end, size and status move with every call while it is
  // being written. They say when to read it again, never that the calls on screen are another run's.
  const runKey = JSON.stringify([source ?? "", agent, runId, turn, from]);
  const requestKey = JSON.stringify([runKey, to, count, status]);

  useEffect(() => {
    if (!open) return;
    if (failed.current?.key === requestKey && failed.current.attempt === attempt) return;
    setError(null);
    // A settled run reads the same every time; one that is running is read again when reopened.
    if (status !== "running" && latest.current?.key === requestKey) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function load() {
      try {
        const steps = await getSteps({ agent, runId, source }, turn, from, to, controller.signal);
        if (controller.signal.aborted) return;
        failed.current = null;
        latest.current = { run: runKey, key: requestKey, steps };
        setLoaded(latest.current);
        // Schedule after completion so slow reads never overlap. A failure stops this loop.
        if (status === "running")
          timer = setTimeout(() => {
            void load();
          }, 1000);
      } catch {
        if (controller.signal.aborted) return;
        failed.current = { key: requestKey, attempt };
        setError(
          latest.current?.run === runKey
            ? "Could not refresh activity details."
            : "Could not load activity details.",
        );
      }
    }
    void load();
    return () => {
      controller.abort();
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [open, attempt, runKey, requestKey, agent, runId, source, turn, from, to, status]);

  // The calls of an earlier read stay up until the next one answers. Swapping them for a loading
  // line, or for a failure, would build every row again and close the ones the reader had opened.
  const steps = loaded?.run === runKey ? loaded.steps : null;
  const tools = steps?.filter((item) => item.kind === "tool");
  const errorCount = tools
    ? tools.filter((item) => item.status === "error").length
    : groupErrorCount(props.stats, turn, from, to);
  const groupStatus = errorCount && status !== "running" ? "error" : activityStatus(status);
  const names = [...new Set(step.tools.map(toolDisplayName))].join(", ");
  const countLabel = tools
    ? `${tools.length} tool ${tools.length === 1 ? "call" : "calls"}`
    : `${step.count} ${step.count === 1 ? "call" : "calls"}`;
  const title = [countLabel, names].filter(Boolean).join(" · ");
  return (
    <ChatToolCall
      title={title}
      status={groupStatus}
      group={{ errorCount }}
      statusLabels={{ cancelled: "No result returned" }}
      open={open}
      onOpenChange={setOpen}
    >
      <div className="px-3 py-2">
        {/* Keyed by a call's place in the turn, so a refresh updates the rows already drawn. */}
        {steps?.map((item, index) => (
          <Activity key={from + index} {...props} step={item} />
        ))}
        {error ? (
          <p className="text-muted-foreground text-xs" role="alert">
            {error}{" "}
            <button
              type="button"
              onClick={() => setAttempt((value) => value + 1)}
              className="text-foreground focus-visible:ring-ring/50 rounded-sm underline underline-offset-2 outline-none focus-visible:ring-2"
            >
              Retry
            </button>
          </p>
        ) : !steps ? (
          <p className="text-muted-foreground text-xs" role="status">
            Loading activity…
          </p>
        ) : (
          !steps.length && (
            <p className="text-muted-foreground text-xs">No activity details available.</p>
          )
        )}
      </div>
    </ChatToolCall>
  );
}

function Activity({ step, ...props }: ActivityProps & { step: AgentStep }) {
  switch (step.kind) {
    case "tool":
      return <ToolActivity {...props} step={step} />;
    case "thinking":
      return (
        <ChatToolCall title="Thinking">
          <ChatToolOutput label="Thinking">{step.text}</ChatToolOutput>
        </ChatToolCall>
      );
    case "calls":
      return <FoldedActivity {...props} step={step} />;
    default:
      return null;
  }
}

export function AppChatAttachments({ turn, source }: { turn: AgentTurn; source?: string }) {
  if (!turn.images.length) return null;
  return (
    <ul aria-label="Attachments" className="text-muted-foreground mt-2 space-y-1 text-xs">
      {turn.images.map((image) => (
        <li key={image.id} className="wrap-anywhere">
          <a
            href={attachmentUrl(image.id, source)}
            target="_blank"
            rel="noreferrer"
            className="focus-visible:ring-ring inline-block rounded-md outline-none focus-visible:ring-2"
          >
            {["image/png", "image/jpeg", "image/webp", "image/gif"].includes(image.media_type) ? (
              <img
                src={attachmentUrl(image.id, source)}
                alt={image.name || "Attached image"}
                loading="lazy"
                className="max-h-48 max-w-full rounded-md object-contain"
              />
            ) : (
              image.name || "Attached file"
            )}
          </a>
        </li>
      ))}
    </ul>
  );
}

function userNote(turn: AgentTurn): string | null {
  for (const marker of turn.markers ?? []) {
    if (marker.kind === "await-woken") return "Agent resumed";
    if (marker.kind === "ask-answered") return "Question answered";
    if (marker.kind === "goal-check") return "Goal check";
  }
  return null;
}

/** Pass these entries directly to ChatThread to let it control message direction. */
export function appChatTranscriptEntries({
  conversation,
  snapshot,
  conversations,
  onOpenConversation,
  onEditMessage,
  editDisabledReason,
}: AppChatTranscriptProps): ReactNode[] {
  const entries: ReactNode[] = [];
  // An answer in flight belongs to the newest message, so nothing can be cut out from under it.
  const editable =
    onEditMessage &&
    snapshot.caps.rewind === true &&
    !snapshot.running &&
    !snapshot.turns.some((turn) => turn.pending);
  snapshot.turns.forEach((turn, index) => {
    if (turn.queued) return;
    const key = JSON.stringify([conversation.agent, conversation.runId, turn.seq]);
    if (turn.role === "user") {
      const note = userNote(turn);
      entries.push(
        <ChatMessage
          key={key}
          from={note ? "system" : "user"}
          data-queued={turn.queued || undefined}
        >
          <div className="min-w-0">
            {note && <ChatMessageMeta>{note}</ChatMessageMeta>}
            {turn.text && (
              <ChatMessageContent className="whitespace-normal">
                <ChatMarkdown>{turn.text}</ChatMarkdown>
              </ChatMessageContent>
            )}
            <AppChatAttachments turn={turn} source={conversation.source} />
          </div>
          {/* Messages sent as one batch reached the agent as one message: only the first can be cut back to. */}
          {editable && !note && !sentWithPrevious(snapshot.turns, index) && (
            <Button
              type="button"
              size="icon"
              variant="ghost"
              aria-label="Edit message"
              title="Edit message"
              data-slot="chat-message-edit"
              className="text-muted-foreground shrink-0 self-end opacity-0 transition-opacity group-focus-within/chat-message:opacity-100 group-hover/chat-message:opacity-100 pointer-coarse:opacity-100"
              disabled={Boolean(editDisabledReason)}
              disabledReason={editDisabledReason ?? ""}
              onClick={() => onEditMessage(turn)}
            >
              <Pencil aria-hidden="true" className="size-3.5" />
            </Button>
          )}
        </ChatMessage>,
      );
      return;
    }
    const parts = transcriptParts(turn);
    if (!parts.length && !turn.pending && !turn.images.length) return;
    entries.push(
      <ChatMessage
        key={key}
        from="assistant"
        className="w-full"
        data-pending={turn.pending || undefined}
      >
        <div className="min-w-0 flex-1 space-y-3">
          {parts.map((part) =>
            part.kind === "message" ? (
              <ChatMessageContent key={part.key} className="whitespace-normal">
                <ChatMarkdown>{part.text}</ChatMarkdown>
              </ChatMessageContent>
            ) : (
              <div key={part.key} className="min-w-0">
                {part.steps.map(({ step, index }) => (
                  <Activity
                    key={index}
                    conversation={conversation}
                    conversations={conversations}
                    onOpenConversation={onOpenConversation}
                    turn={turn.seq}
                    stats={snapshot.stats}
                    step={step}
                  />
                ))}
              </div>
            ),
          )}
          <AppChatAttachments turn={turn} source={conversation.source} />
          {turn.pending && <ChatTypingIndicator label="Agent is responding" />}
        </div>
      </ChatMessage>,
    );
  });
  if (!entries.length && !snapshot.turns.some((turn) => turn.queued))
    entries.push(
      <ChatMessage key="empty" from={snapshot.output ? "assistant" : "system"} className="w-full">
        {snapshot.output ? (
          <ChatToolOutput label="Agent output">{snapshot.output}</ChatToolOutput>
        ) : (
          <ChatMessageContent>
            {snapshot.running ? "Working…" : "No messages yet."}
          </ChatMessageContent>
        )}
      </ChatMessage>,
    );
  return entries;
}

export function AppChatTranscript(props: AppChatTranscriptProps) {
  return appChatTranscriptEntries(props);
}
