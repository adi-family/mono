import { useEffect, useRef, useState } from "react";
import { CornerDownLeft, RotateCcw } from "lucide-react";
import { useNavigate } from "react-router-dom";

import {
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  ChatComposer,
  ChatQueuedMessage,
  ChatWaitingStatus,
  ChatMessage,
  ChatMessageActions,
  ChatMessageContent,
  ChatMessageMeta,
  ChatThread,
  ChatToolCall,
  ChatToolDiff,
  ChatToolOutput,
  ChatTypingIndicator,
  Select,
  type ChatThreadDirection,
  type ChatToolCallStatus,
} from "@design-system/ui";

import { CodeExample } from "./CodeExample";

type Message = { id: number; from: "user" | "assistant"; text: string; tools?: boolean };
const initialMessages: Message[] = [
  { id: 1, from: "user", text: "Can you check the chat layout and show what changed?" },
  { id: 2, from: "assistant", text: "I checked the working tree and updated the chat spacing. The typecheck is still running.", tools: true },
];
const reply = "Let’s make that the next step. Keep the conversation easy to scan, give every action a clear label, and keep the draft safe if sending fails.\n\nYou can scroll back to earlier messages while this response arrives, then jump to the latest when you’re ready.";

const gitOutput = `## feature/chat-tools
 M src/components/chat-panel.tsx
 M src/styles.css`;

const layoutDiff = [
  { type: "context", content: "export function ChatPanel() {", oldLineNumber: 16, newLineNumber: 16 },
  { type: "context", content: "  return (", oldLineNumber: 17, newLineNumber: 17 },
  { type: "deletion", content: '    <div className="flex flex-col gap-2">', oldLineNumber: 18 },
  { type: "addition", content: '    <div className="flex flex-col gap-4">', newLineNumber: 18 },
  { type: "context", content: "      <ChatMessages />", oldLineNumber: 19, newLineNumber: 19 },
] as const;

const tokenDiff = [
  { type: "context", content: ":root {", oldLineNumber: 1, newLineNumber: 1 },
  { type: "deletion", content: "  --chat-content-width: 40rem;", oldLineNumber: 2 },
  { type: "addition", content: "  --chat-content-width: 48rem;", newLineNumber: 2 },
  { type: "context", content: "}", oldLineNumber: 3, newLineNumber: 3 },
] as const;

const terminalOutput: Record<ChatToolCallStatus, string> = {
  pending: "Waiting for a terminal…",
  running: "$ tsc -b --noEmit\nChecking project references…",
  completed: "$ tsc -b --noEmit\n\nTypecheck passed. Exit code 0.",
  error: "$ tsc -b --noEmit\nsrc/components/chat-panel.tsx(24,7): error TS2322:\n  Type 'string' is not assignable to type 'number'.\n\nExit code 2.",
  cancelled: "$ tsc -b --noEmit\n^C\nCheck cancelled before completion.",
};

function AgentActivity({ status }: { status: ChatToolCallStatus }) {
  const navigate = useNavigate();

  return (
    <ChatToolCall
      kind="agent"
      title="Layout reviewer · Check chat layout"
      status={status}
      href="/library/agents/layout-reviewer"
      linkProps={{
        onClick: (event) => {
          if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
          event.preventDefault();
          navigate("/library/agents/layout-reviewer");
        },
      }}
    >
      <div className="px-3 py-2 text-xs">
        <p>Check the chat spacing and scan the layout on narrow screens.</p>
        <p className="mt-1 text-muted-foreground">
          {status === "completed"
            ? "The message spacing is consistent and the layout fits mobile screens."
            : "Reviewing the updated conversation layout."}
        </p>
      </div>
    </ChatToolCall>
  );
}

function ConversationTools() {
  return (
    <div className="mb-2 flex min-w-0 flex-col gap-0">
      <AgentActivity status="completed" />
      <ChatToolCall kind="git" title="exec_command" description="git status --short --branch" status="completed" meta="Exit 0">
        <ChatToolOutput label="Git output">{gitOutput}</ChatToolOutput>
      </ChatToolCall>
      <ChatToolCall kind="edit" title="apply_patch" description="Update chat spacing" status="completed" meta="2 files">
        <ChatToolDiff file="src/components/chat-panel.tsx" lines={layoutDiff} />
        <ChatToolDiff file="src/styles.css" lines={tokenDiff} />
      </ChatToolCall>
      <ChatToolCall kind="terminal" title="exec_command" description="bun run typecheck" status="running" meta="frontend">
        <ChatToolOutput label="Terminal output">{terminalOutput.running}</ChatToolOutput>
      </ChatToolCall>
    </div>
  );
}

const threadCode = `// direction: "top-to-bottom" (default) | "bottom-to-top"
const parts = [
  <ChatThread key="messages" label="Design conversation" direction={direction}>
    {messages.map((message) => (
      <ChatMessage key={message.id} from={message.from}>
        <div className="min-w-0">
          <ChatMessageMeta>{message.speaker}</ChatMessageMeta>
          <ChatMessageContent>{message.text}</ChatMessageContent>
        </div>
      </ChatMessage>
    ))}
  </ChatThread>,
  <ChatComposer
    key="composer"
    value={draft}
    onValueChange={setDraft}
    onSend={sendMessage}
    isGenerating={isGenerating}
    onStop={stopResponse}
  />,
];

<div className="flex h-[32rem] flex-col rounded-xl border">
  {direction === "bottom-to-top" ? parts.reverse() : parts}
</div>`;

const messageCode = `<ChatMessage from="assistant">
  <div className="min-w-0">
    <ChatMessageMeta>Assistant · Just now</ChatMessageMeta>
    <ChatMessageContent>Your response goes here.</ChatMessageContent>
    <ChatMessageActions>
      <Button variant="ghost" size="sm" onClick={useAsDraft}>
        Use as draft
      </Button>
    </ChatMessageActions>
  </div>
</ChatMessage>

<ChatTypingIndicator label="Assistant is preparing a reply" />`;

const composerCode = `const [draft, setDraft] = useState("");

<ChatComposer
  value={draft}
  onValueChange={setDraft}
  onSend={async (message) => {
    await sendMessage(message);
  }}
  textareaProps={{ name: "message" }}
/>`;

const toolsCode = `<ChatToolCall kind="terminal" title="exec_command" description="bun run typecheck" status="running">
  <ChatToolOutput label="Terminal output">{output}</ChatToolOutput>
</ChatToolCall>

<ChatToolCall title="8 tool calls · exec_command, apply_patch" status="error" group={{ errorCount: 1 }}>
  <div className="px-3 py-1">
    <ChatToolCall title="exec_command" description="git status --short" kind="git" status="completed" />
    <ChatToolCall title="exec_command" description="bun run lint" kind="terminal" status="error" />
    {/* Remaining calls */}
  </div>
</ChatToolCall>

<ChatToolCall
  kind="agent"
  title="Layout reviewer · Check chat layout"
  status="running"
  href="/library/agents/layout-reviewer"
>
  <div className="px-3 py-2 text-xs">Check spacing and mobile layout.</div>
</ChatToolCall>

<ChatToolCall kind="edit" title="apply_patch" description="Update chat spacing" status="completed">
  <ChatToolDiff file="src/chat.tsx" lines={[
    { type: "deletion", content: 'gap-2', oldLineNumber: 18 },
    { type: "addition", content: 'gap-4', newLineNumber: 18 },
  ]} />
</ChatToolCall>`;

function ToolCallsExample() {
  const [status, setStatus] = useState<ChatToolCallStatus>("running");
  const replayTimers = useRef<ReturnType<typeof setTimeout>[]>([]);

  function stopReplay() {
    replayTimers.current.forEach(clearTimeout);
    replayTimers.current = [];
  }

  useEffect(() => () => stopReplay(), []);

  function replayCheck() {
    stopReplay();
    setStatus("pending");
    replayTimers.current = [
      setTimeout(() => setStatus("running"), 600),
      setTimeout(() => setStatus("completed"), 2400),
    ];
  }

  return (
    <Card id="chat-tools-example" tabIndex={-1} className="scroll-mt-36 md:scroll-mt-8">
      <CardHeader>
        <CardTitle>Tool calls</CardTitle>
        <CardDescription>Quiet, expandable activity for agents, Git, commands, and file changes. Simulated examples.</CardDescription>
      </CardHeader>
      <CardContent className="flex min-w-0 flex-col gap-4 px-3 sm:px-6">
        <div className="flex flex-wrap items-center gap-3">
          <label htmlFor="chat-tool-status" className="text-sm font-medium">Terminal status</label>
          <div className="w-full sm:w-44">
            <Select
              id="chat-tool-status"
              value={status}
              onValueChange={(value) => { stopReplay(); setStatus(value as ChatToolCallStatus); }}
              options={[
                { value: "pending", label: "Pending" },
                { value: "running", label: "Running" },
                { value: "completed", label: "Completed" },
                { value: "error", label: "Error" },
                { value: "cancelled", label: "Cancelled" },
              ]}
            />
          </div>
          <Button type="button" variant="outline" size="sm" onClick={replayCheck}>
            <RotateCcw aria-hidden="true" /> Replay check
          </Button>
        </div>
        <div className="min-w-0">
          <ChatMessage from="assistant" className="w-full">
            <div className="flex w-full min-w-0 flex-col gap-0">
              <AgentActivity status="running" />
              <ChatToolCall kind="git" title="exec_command" description="git status --short --branch" status="completed" meta="Exit 0">
                <ChatToolOutput label="Git output">{gitOutput}</ChatToolOutput>
              </ChatToolCall>
              <ChatToolCall kind="edit" title="apply_patch" description="Update chat spacing" status="completed" meta="2 files · +2 −2">
                <ChatToolDiff file="src/components/chat-panel.tsx" lines={layoutDiff} />
                <ChatToolDiff file="src/styles.css" lines={tokenDiff} />
              </ChatToolCall>
              <ChatToolCall
                kind="terminal"
                title="exec_command"
                status={status}
                description="bun run typecheck · frontend"
                data-demo="terminal-status"
              >
                <ChatToolOutput label="Terminal output">{terminalOutput[status]}</ChatToolOutput>
              </ChatToolCall>
              <ChatToolCall title="8 tool calls · exec_command, Read, apply_patch" status="error" group={{ errorCount: 1 }} data-demo="grouped-tool-errors">
                <div className="px-3 py-1">
                  <ChatToolCall title="exec_command" description="git diff --stat" kind="git" status="completed" />
                  <ChatToolCall title="Read" description="chat-panel.tsx" status="completed" />
                  <ChatToolCall title="Read" description="styles.css" status="completed" />
                  <ChatToolCall title="apply_patch" description="Update message width" kind="edit" status="completed" />
                  <ChatToolCall title="exec_command" description="bun run typecheck" kind="terminal" status="completed" />
                  <ChatToolCall title="exec_command" description="bun run lint" kind="terminal" status="error">
                    <ChatToolOutput label="Lint error">{"src/chat-panel.tsx:24:7\n'previousWidth' is assigned a value but never used.\n\nExit code 1."}</ChatToolOutput>
                  </ChatToolCall>
                  <ChatToolCall title="apply_patch" description="Remove unused variable" kind="edit" status="completed" />
                  <ChatToolCall title="exec_command" description="bun run build" kind="terminal" status="completed" />
                </div>
              </ChatToolCall>
            </div>
          </ChatMessage>
        </div>
        <p className="text-muted-foreground text-sm">Tool names stay compact; expand for commands and details. Groups highlight their error count; failed commands stay red. Replay cycles through the command states.</p>
        <CodeExample code={toolsCode} label="Tool calls" />
      </CardContent>
    </Card>
  );
}

export function ChatExamples() {
  const [messages, setMessages] = useState(initialMessages);
  const [draft, setDraft] = useState("");
  const [isGenerating, setIsGenerating] = useState(false);
  const [direction, setDirection] = useState<ChatThreadDirection>("top-to-bottom");
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);
  const nextId = useRef(3);
  const [testDraft, setTestDraft] = useState("");
  const [simulateFailure, setSimulateFailure] = useState(false);
  const [lastSent, setLastSent] = useState("");
  const [locked, setLocked] = useState(false);

  useEffect(() => () => { if (timer.current) clearInterval(timer.current); }, []);

  function stopResponse() {
    if (timer.current) clearInterval(timer.current);
    timer.current = null;
    setIsGenerating(false);
    setMessages((current) => current.map((message) =>
      message.from === "assistant" && !message.text ? { ...message, text: "Response stopped." } : message,
    ));
  }

  function sendMessage(text: string) {
    const userId = nextId.current++;
    const assistantId = nextId.current++;
    setMessages((current) => [...current, { id: userId, from: "user", text }, { id: assistantId, from: "assistant", text: "" }]);
    setIsGenerating(true);
    let offset = 0;
    timer.current = setInterval(() => {
      offset = Math.min(offset + 5, reply.length);
      setMessages((current) => current.map((message) => message.id === assistantId ? { ...message, text: reply.slice(0, offset) } : message));
      if (offset === reply.length) stopResponse();
    }, 55);
  }

  const conversationParts = [
    <ChatThread key="messages" label="Design conversation" direction={direction} aria-live="off">
      <ChatMessage from="system"><ChatMessageContent>Today</ChatMessageContent></ChatMessage>
      {messages.map((message) => (
        <ChatMessage key={message.id} from={message.from}>
          <div className="min-w-0">
            <ChatMessageMeta>{message.from === "assistant" ? "Design assistant" : "You"}</ChatMessageMeta>
            {message.tools && <ConversationTools />}
            <ChatMessageContent>
              {message.text || (isGenerating ? <ChatTypingIndicator /> : "Response stopped.")}
            </ChatMessageContent>
            {message.from === "assistant" && message.text && (
              <ChatMessageActions>
                <Button type="button" variant="ghost" size="sm" onClick={() => setDraft(message.text)}>
                  <CornerDownLeft className="size-3.5" aria-hidden="true" /> Use as draft
                </Button>
              </ChatMessageActions>
            )}
          </div>
        </ChatMessage>
      ))}
    </ChatThread>,
    <div key="composer" className={direction === "bottom-to-top" ? "shrink-0 border-b p-3" : "shrink-0 border-t p-3"}>
      <ChatComposer
        value={draft}
        onValueChange={setDraft}
        onSend={sendMessage}
        isGenerating={isGenerating}
        onStop={stopResponse}
        inputLabel="Message the design assistant"
        className="bg-muted/30"
      />
      <p className="sr-only" role="status">{isGenerating ? "Assistant is responding." : "Ready for your message."}</p>
    </div>,
  ];

  return (
    <>
      <Card id="chat-thread-example" tabIndex={-1} className="scroll-mt-36 md:scroll-mt-8">
        <CardHeader>
          <CardTitle>Chat conversation</CardTitle>
          <CardDescription>
            Messages, tool activity, and live responses in one conversation. Send a message to try it. Tool calls are simulated.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex min-w-0 flex-col gap-4 px-3 sm:px-6">
          <div className="flex flex-wrap items-center gap-3">
            <label htmlFor="chat-direction" className="text-sm font-medium">Message direction</label>
            <div className="w-full sm:w-48">
              <Select
                id="chat-direction"
                value={direction}
                onValueChange={(value) => setDirection(value as ChatThreadDirection)}
                options={[
                  { value: "top-to-bottom", label: "Top to bottom" },
                  { value: "bottom-to-top", label: "Bottom to top" },
                ]}
              />
            </div>
          </div>
          <div className="bg-background flex h-[34rem] min-w-0 flex-col overflow-hidden rounded-xl border">
            {direction === "bottom-to-top" ? conversationParts.reverse() : conversationParts}
          </div>
          <p className="text-muted-foreground text-sm">
            {direction === "top-to-bottom"
              ? "Messages run from oldest to newest, with new replies at the bottom."
              : "Messages run from newest to oldest, with new replies at the top."}
            {" "}Scroll to read earlier messages; use Jump to latest to return to the newest reply.
          </p>
          <CodeExample code={threadCode} label="Chat conversation" />
        </CardContent>
      </Card>

      <ToolCallsExample />

      <Card id="chat-messages-example" tabIndex={-1} className="scroll-mt-36 md:scroll-mt-8">
        <CardHeader>
          <CardTitle>Chat messages</CardTitle>
          <CardDescription>User, assistant, and system messages with details and actions.</CardDescription>
        </CardHeader>
        <CardContent className="flex min-w-0 flex-col gap-4 px-3 sm:px-6">
          <div className="flex min-w-0 flex-col gap-6 rounded-xl border p-4">
            <ChatMessage from="system"><ChatMessageContent>A new conversation has started</ChatMessageContent></ChatMessage>
            <ChatMessage from="user">
              <div className="min-w-0">
                <ChatMessageMeta>You <time dateTime="2026-11-02T09:41">09:41</time></ChatMessageMeta>
                <ChatMessageContent>Can we keep this simple?</ChatMessageContent>
              </div>
            </ChatMessage>
            <ChatMessage from="assistant">
              <div className="min-w-0">
                <ChatMessageMeta>Design assistant <time dateTime="2026-11-02T09:41">09:41</time></ChatMessageMeta>
                <ChatMessageContent>Absolutely. Start with the essentials and leave room to grow.</ChatMessageContent>
                <ChatMessageActions>
                  <Button type="button" variant="ghost" size="sm" onClick={() => setTestDraft("Absolutely. Start with the essentials and leave room to grow.")}>
                    <CornerDownLeft className="size-3.5" aria-hidden="true" /> Use in composer below
                  </Button>
                </ChatMessageActions>
              </div>
            </ChatMessage>
            <div className="flex items-center gap-3">
              <ChatTypingIndicator />
              <span className="text-muted-foreground text-xs">Preparing a reply</span>
            </div>
          </div>
          <CodeExample code={messageCode} label="Chat messages" />
        </CardContent>
      </Card>

      <Card id="chat-composer-example" tabIndex={-1} className="scroll-mt-36 md:scroll-mt-8">
        <CardHeader>
          <CardTitle>Chat composer</CardTitle>
          <CardDescription>Multiline drafts, keyboard submission, and a retryable send error that keeps your text.</CardDescription>
        </CardHeader>
        <CardContent className="flex min-w-0 flex-col gap-4 px-3 sm:px-6">
          <div className="flex flex-wrap gap-x-6 gap-y-3">
            <label className="flex items-center gap-2 text-sm">
              <input type="checkbox" checked={simulateFailure} onChange={(event) => setSimulateFailure(event.target.checked)} className="accent-primary size-4" />
              Simulate a failed send
            </label>
            <label className="flex items-center gap-2 text-sm">
              <input type="checkbox" checked={locked} onChange={(event) => setLocked(event.target.checked)} className="accent-primary size-4" />
              Read-only conversation
            </label>
          </div>
          <ChatComposer
            value={testDraft}
            onValueChange={setTestDraft}
            inputLabel="Try the chat composer"
            disabled={locked}
            disabledReason="This conversation is read-only. Turn off the preview option to send a message."
            onSend={async (message) => {
              setLastSent("");
              await new Promise((resolve) => setTimeout(resolve, 600));
              if (simulateFailure) throw new Error("Preview send failure");
              setLastSent(message);
            }}
          />
          <p role="status" className={lastSent ? "text-muted-foreground text-sm [overflow-wrap:anywhere]" : "sr-only"}>
            {lastSent ? `Message sent: ${lastSent}` : ""}
          </p>
          <ChatWaitingStatus items={[
            { id: "build", label: "Waiting for the build to finish." },
            { id: "review", label: "Waiting for the layout reviewer’s result." },
          ]} />
          <ChatQueuedMessage text="Please also check the mobile layout after this response." />
          <p className="text-muted-foreground text-xs">Queued messages stay compact until expanded. In the live Interface, attach images using the paperclip, paste, or drag and drop.</p>
          <CodeExample code={composerCode} label="Chat composer" />
        </CardContent>
      </Card>
    </>
  );
}
