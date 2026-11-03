import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  ChatMessage,
  ChatMessageContent,
  ChatMessageMeta,
  ChatToolCall,
  ChatToolOutput,
  ChatTypingIndicator,
} from "@design-system/ui";
import { attachmentUrl, getSteps, type AgentPeek, type AgentStep, type AgentTurn, type Conversation, type ConversationRef } from "../lib/app-chat";
import { activityStatus, agentLaunches, conversationHref, toolArguments, toolDisplayName, toolSummary, transcriptParts } from "../lib/app-chat-transcript";
import { groupErrorCount } from "../lib/app-chat-activity";
import { ChatMarkdown } from "./ChatMarkdown";
import { conversationKey, sameConversation } from "../lib/app-chat-sources";

export interface AppChatTranscriptProps {
  conversation: ConversationRef;
  snapshot: AgentPeek;
  conversations: Conversation[];
  onOpenConversation: (ref: ConversationRef) => void;
}

type ActivityProps = Omit<AppChatTranscriptProps, "snapshot"> & { turn: number; stats?: AgentPeek["stats"] };
type ToolStep = Extract<AgentStep, { kind: "tool" }>;
type FoldedStep = Extract<AgentStep, { kind: "calls" }>;

function ToolArguments({ input }: { input: string }) {
  const args = toolArguments(input);
  if (!args.length) return null;
  return (
    <dl className="space-y-2 px-3 py-2 text-xs">
      {args.map(({ name, value }) => {
        const lengthy = value.length > 400 || value.split("\n").length > 12;
        return <div key={name}>
          <dt className="text-muted-foreground mb-0.5">{name}</dt>
          <dd
            tabIndex={lengthy ? 0 : undefined}
            role={lengthy ? "region" : undefined}
            aria-label={lengthy ? `${name} argument` : undefined}
            className="focus-visible:ring-ring/50 m-0 max-h-64 overflow-auto overscroll-contain rounded-sm font-mono whitespace-pre-wrap outline-none focus-visible:ring-2 focus-visible:ring-inset [overflow-wrap:anywhere]"
          >{value}</dd>
        </div>;
      })}
    </dl>
  );
}

function ToolActivity({ step, conversation, conversations, onOpenConversation }: ActivityProps & { step: ToolStep }) {
  const children = agentLaunches(step).map((ref) => ({ ...ref, ...(conversation.source ? { source: conversation.source } : {}) }));
  const details = <>
    <ToolArguments input={step.input} />
    {step.output && <ChatToolOutput>{step.output}</ChatToolOutput>}
    {!step.output && step.status === "running" && <p className="text-muted-foreground px-3 py-2 text-xs">Waiting for output…</p>}
  </>;
  if (children.length) return children.map((ref) => {
    const child = conversations.find((item) => sameConversation(item, ref));
    return (
      <ChatToolCall
        key={conversationKey(ref)}
        kind="agent"
        title={[ref.agent, child?.title].filter(Boolean).join(" · ")}
        status={child?.running ? "running" : child?.state === "waiting" ? "pending" : "completed"}
        statusLabels={{ completed: child ? "Conversation available" : "Started" }}
        href={conversationHref(ref)}
        linkProps={{ onClick: (event) => {
          if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
          event.preventDefault();
          onOpenConversation(ref);
        } }}
      >
        {details}
      </ChatToolCall>
    );
  });
  const summary = toolSummary(step.name, step.input);
  return (
    <ChatToolCall {...summary} status={activityStatus(step.status)} statusLabels={{ cancelled: "No result returned" }}>
      {details}
    </ChatToolCall>
  );
}

function FoldedActivity({ step, ...props }: ActivityProps & { step: FoldedStep }) {
  const [open, setOpen] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [loaded, setLoaded] = useState<{ key: string; steps: AgentStep[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const cache = useRef<{ key: string; steps: AgentStep[] } | null>(null);
  const failed = useRef<{ key: string; attempt: number } | null>(null);
  const { agent, runId, source } = props.conversation;
  const { turn } = props;
  const { from, to, count, status } = step;
  const requestKey = JSON.stringify([source ?? "", agent, runId, turn, from, to, count, status]);

  useEffect(() => {
    if (!open) return;
    if (failed.current?.key === requestKey && failed.current.attempt === attempt) return;
    setError(null);
    const cached = cache.current;
    if (status !== "running" && cached?.key === requestKey) {
      setLoaded(cached);
      return;
    }
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function load() {
      try {
        const steps = await getSteps({ agent, runId, source }, turn, from, to, controller.signal);
        if (controller.signal.aborted) return;
        const next = { key: requestKey, steps };
        cache.current = status === "running" ? null : next;
        failed.current = null;
        setLoaded(next);
        // Schedule after completion so slow reads never overlap. A failure stops this loop.
        if (status === "running") timer = setTimeout(() => { void load(); }, 1000);
      } catch {
        if (controller.signal.aborted) return;
        failed.current = { key: requestKey, attempt };
        setError("Could not load activity details.");
      }
    }
    void load();
    return () => {
      controller.abort();
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [open, attempt, requestKey, agent, runId, source, turn, from, to, status]);

  const steps = loaded?.key === requestKey ? loaded.steps : null;
  const tools = steps?.filter((item) => item.kind === "tool");
  const errorCount = tools
    ? tools.filter((item) => item.status === "error").length
    : groupErrorCount(props.stats, turn, from, to);
  const groupStatus = errorCount && status !== "running" ? "error" : activityStatus(status);
  const names = [...new Set(step.tools.map(toolDisplayName))].join(", ");
  const countLabel = tools ? `${tools.length} tool ${tools.length === 1 ? "call" : "calls"}` : `${step.count} ${step.count === 1 ? "call" : "calls"}`;
  const title = [countLabel, names].filter(Boolean).join(" · ");
  return (
    <ChatToolCall title={title} status={groupStatus} group={{ errorCount }} statusLabels={{ cancelled: "No result returned" }} open={open} onOpenChange={setOpen}>
      <div className="px-3 py-2">
        {error ? (
          <p className="text-muted-foreground text-xs" role="alert">
            {error}{" "}
            <button type="button" onClick={() => setAttempt((value) => value + 1)} className="text-foreground focus-visible:ring-ring/50 rounded-sm underline underline-offset-2 outline-none focus-visible:ring-2">Retry</button>
          </p>
        ) : !steps ? <p className="text-muted-foreground text-xs" role="status">Loading activity…</p>
          : !steps.length ? <p className="text-muted-foreground text-xs">No activity details available.</p>
            : steps.map((item, index) => <Activity key={from + index} {...props} step={item} />)}
      </div>
    </ChatToolCall>
  );
}

function Activity({ step, ...props }: ActivityProps & { step: AgentStep }) {
  switch (step.kind) {
    case "tool": return <ToolActivity {...props} step={step} />;
    case "thinking": return <ChatToolCall title="Thinking"><ChatToolOutput label="Thinking">{step.text}</ChatToolOutput></ChatToolCall>;
    case "calls": return <FoldedActivity {...props} step={step} />;
    default: return null;
  }
}

export function AppChatAttachments({ turn, source }: { turn: AgentTurn; source?: string }) {
  if (!turn.images.length) return null;
  return (
    <ul aria-label="Attachments" className="text-muted-foreground mt-2 space-y-1 text-xs">
      {turn.images.map((image) => <li key={image.id} className="[overflow-wrap:anywhere]">
        <a href={attachmentUrl(image.id, source)} target="_blank" rel="noreferrer" className="focus-visible:ring-ring inline-block rounded-md outline-none focus-visible:ring-2">
          {["image/png", "image/jpeg", "image/webp", "image/gif"].includes(image.media_type)
            ? <img src={attachmentUrl(image.id, source)} alt={image.name || "Attached image"} loading="lazy" className="max-h-48 max-w-full rounded-md object-contain" />
            : image.name || "Attached file"}
        </a>
      </li>)}
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
export function appChatTranscriptEntries({ conversation, snapshot, conversations, onOpenConversation }: AppChatTranscriptProps): ReactNode[] {
  const entries: ReactNode[] = [];
  snapshot.turns.forEach((turn) => {
    if (turn.queued) return;
    const key = JSON.stringify([conversation.agent, conversation.runId, turn.seq]);
    if (turn.role === "user") {
      const note = userNote(turn);
      entries.push(
        <ChatMessage key={key} from={note ? "system" : "user"} data-queued={turn.queued || undefined}>
          <div className="min-w-0">
            {note && <ChatMessageMeta>{note}</ChatMessageMeta>}
            {turn.text && <ChatMessageContent className="whitespace-normal"><ChatMarkdown>{turn.text}</ChatMarkdown></ChatMessageContent>}
            <AppChatAttachments turn={turn} source={conversation.source} />
          </div>
        </ChatMessage>,
      );
      return;
    }
    const parts = transcriptParts(turn);
    if (!parts.length && !turn.pending && !turn.images.length) return;
    entries.push(
      <ChatMessage key={key} from="assistant" className="w-full" data-pending={turn.pending || undefined}>
        <div className="min-w-0 flex-1 space-y-3">
          {parts.map((part) => part.kind === "message"
            ? <ChatMessageContent key={part.key} className="whitespace-normal"><ChatMarkdown>{part.text}</ChatMarkdown></ChatMessageContent>
            : <div key={part.key} className="min-w-0">{part.steps.map(({ step, index }) => (
              <Activity key={index} conversation={conversation} conversations={conversations} onOpenConversation={onOpenConversation} turn={turn.seq} stats={snapshot.stats} step={step} />
            ))}</div>)}
          <AppChatAttachments turn={turn} source={conversation.source} />
          {turn.pending && <ChatTypingIndicator label="Agent is responding" />}
        </div>
      </ChatMessage>,
    );
  });
  if (!entries.length && !snapshot.turns.some((turn) => turn.queued)) entries.push(
    <ChatMessage key="empty" from={snapshot.output ? "assistant" : "system"} className="w-full">
      {snapshot.output ? <ChatToolOutput label="Agent output">{snapshot.output}</ChatToolOutput>
        : <ChatMessageContent>{snapshot.running ? "Working…" : "No messages yet."}</ChatMessageContent>}
    </ChatMessage>,
  );
  return entries;
}

export function AppChatTranscript(props: AppChatTranscriptProps) {
  return appChatTranscriptEntries(props);
}
