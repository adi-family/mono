import { useState } from "react";
import { createRoot } from "react-dom/client";

import {
  ChatComposer,
  ChatMessage,
  ChatMessageActions,
  ChatMessageAvatar,
  ChatMessageContent,
  ChatMessageMeta,
  ChatThread,
  ChatToolCall,
  ChatToolDiff,
  ChatToolOutput,
  ChatTypingIndicator,
  type ChatThreadDirection,
  type ChatToolCallStatus,
  type ChatToolDiffLine,
} from "../src/components";

const command = 'git diff -- "<chat>&.tsx" && printf "$USER is literal"';
const output = '<script>window.__toolMarkupExecuted = true</script>\nfile: <chat>&.tsx\n' + "long-output-".repeat(80);
const agentTitle = "Layout reviewer · Check chat layout";
const agentResult = "The compact activity row fits <mobile> widths & keeps details optional.";
const diffLines: readonly ChatToolDiffLine[] = [
  { type: "context", content: "function render() {", oldLineNumber: 9, newLineNumber: 9 },
  { type: "deletion", content: '  return "<script>old</script>";', oldLineNumber: 10 },
  { type: "addition", content: '  return "<script>new</script> & safe";', newLineNumber: 10 },
  { type: "addition", content: '  // ' + "long-diff-".repeat(80), newLineNumber: 11 },
  { type: "context", content: "}", oldLineNumber: 11, newLineNumber: 12 },
];

const state = {
  command,
  output,
  agentTitle,
  agentResult,
  agentLinkActivations: 0,
  diffLines,
  toolOpenRequests: [] as boolean[],
  sent: [] as string[],
  stops: 0,
  keydowns: 0,
  preventEnter: false,
  resolve: () => {},
  reject: () => {},
  setDisabled: (_disabled: boolean) => {},
  setGenerating: (_generating: boolean) => {},
  setSendOnEnter: (_sendOnEnter: boolean) => {},
  setDraft: (_draft: string) => {},
  setDirection: (_direction: ChatThreadDirection) => {},
  setManualDirection: (_direction: ChatThreadDirection) => {},
  setToolStatus: (_status: ChatToolCallStatus) => {},
  setToolOpen: (_open: boolean) => {},
  setAgentStatus: (_status: ChatToolCallStatus) => {},
  append: () => {},
};

function FixtureMessage({ message, index }: { message: string; index: number }) {
  const [count, setCount] = useState(0);
  return (
    <ChatMessage from={index % 2 ? "user" : "assistant"} data-message-number={index + 1}>
      <ChatMessageAvatar>{index % 2 ? "You" : "AI"}</ChatMessageAvatar>
      <div style={{ minWidth: 0 }}>
        <ChatMessageMeta>{index % 2 ? "You" : "Assistant"} · Just now</ChatMessageMeta>
        <ChatMessageContent>{message}</ChatMessageContent>
        {index === 0 && <button id="test-message-state" type="button" onClick={() => setCount((value) => value + 1)}>Saved reactions: {count}</button>}
      </div>
    </ChatMessage>
  );
}

function Fixture() {
  const [draft, setDraft] = useState("");
  const [disabled, setDisabled] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [sendOnEnter, setSendOnEnter] = useState(true);
  const [direction, setDirection] = useState<ChatThreadDirection>();
  const [manualDirection, setManualDirection] = useState<ChatThreadDirection>("bottom-to-top");
  const [toolStatus, setToolStatus] = useState<ChatToolCallStatus>("pending");
  const [toolOpen, setToolOpen] = useState(false);
  const [agentStatus, setAgentStatus] = useState<ChatToolCallStatus>("completed");
  const [messages, setMessages] = useState(Array.from({ length: 18 }, (_, index) => `Message ${index + 1}: ${"A long chat message that wraps safely. ".repeat(3)}`));

  Object.assign(state, {
    setDisabled,
    setGenerating,
    setSendOnEnter,
    setDraft,
    setDirection,
    setManualDirection,
    setToolStatus,
    setToolOpen,
    setAgentStatus,
    append: () => setMessages((current) => [...current, `Message ${current.length + 1}: ${"Appended content. ".repeat(12)}`]),
  });

  function send(message: string) {
    state.sent.push(message);
    return new Promise<void>((resolve, reject) => {
      state.resolve = resolve;
      state.reject = () => reject(new Error("Connection lost. Please try again."));
    });
  }

  return (
    <main style={{ boxSizing: "border-box", width: "100%", maxWidth: 640, padding: 24, margin: "0 auto", display: "grid", gap: 20 }}>
      <h1>Chat browser fixtures</h1>
      <ChatThread id="test-thread" label="Test conversation" direction={direction} style={{ height: 280 }}>
        {messages.map((message, index) => (
          <FixtureMessage key={index} message={message} index={index} />
        ))}
        <div id="test-growing-content" style={{ height: 1 }} />
      </ChatThread>
      <ChatComposer
        id="test-composer"
        value={draft}
        onValueChange={setDraft}
        onSend={send}
        disabled={disabled}
        disabledReason="Your connection must be restored before sending."
        isGenerating={generating}
        sendOnEnter={sendOnEnter}
        onStop={() => { state.stops++; setGenerating(false); }}
        textareaProps={{
          id: "test-draft",
          name: "message",
          onKeyDown: (event) => {
            state.keydowns++;
            if (state.preventEnter && event.key === "Enter") event.preventDefault();
          },
        }}
      >
        <button id="test-toolbar" type="button">Attach a file</button>
      </ChatComposer>
      <output id="test-draft-value">{draft}</output>
      <ChatThread id="test-manual-thread" label="Manual conversation" autoScroll={false} style={{ height: 180 }}>
        {messages.map((message, index) => (
          <ChatMessage from="assistant" key={index}><ChatMessageContent>{message}</ChatMessageContent></ChatMessage>
        ))}
      </ChatThread>
      <ChatThread id="test-manual-reverse-thread" label="Manual reverse conversation" direction={manualDirection} autoScroll={false} style={{ height: 180 }}>
        {messages.map((message, index) => (
          <ChatMessage from="assistant" key={index} data-message-number={index + 1}><ChatMessageContent>{message}</ChatMessageContent></ChatMessage>
        ))}
      </ChatThread>
      <ChatMessage id="test-overflow-message" from="assistant">
        <ChatMessageAvatar>AI</ChatMessageAvatar>
        <div style={{ minWidth: 0 }}>
          <ChatMessageMeta>Assistant</ChatMessageMeta>
          <ChatMessageContent>{"A".repeat(400)}</ChatMessageContent>
          <ChatMessageActions><button id="test-copy" type="button">Copy message</button></ChatMessageActions>
        </div>
      </ChatMessage>
      <ChatMessage from="system"><ChatMessageContent>Conversation started</ChatMessageContent></ChatMessage>
      <ChatTypingIndicator id="test-typing" />
      <section id="test-tool-calls" aria-label="Tool call fixtures" style={{ minWidth: 0, display: "grid", gap: 12 }}>
        <ChatToolCall id="test-tool-disclosure" title={command} kind="terminal" status={toolStatus} meta="0.4 s">
          <ChatToolOutput id="test-tool-output" label="Verification output">{output}</ChatToolOutput>
        </ChatToolCall>
        <ChatToolCall
          id="test-tool-controlled"
          title="Inspect workspace"
          description="git status --short"
          kind="git"
          status={toolStatus}
          open={toolOpen}
          onOpenChange={(open) => { state.toolOpenRequests.push(open); }}
        >
          <ChatToolOutput>{" M packages/ui/src/components/chat.tsx"}</ChatToolOutput>
        </ChatToolCall>
        <ChatToolCall id="test-tool-edit" title="Update chat rendering" kind="edit" defaultOpen>
          <ChatToolDiff id="test-tool-diff" file={"packages/ui/src/components/" + "chat-".repeat(20) + ".tsx"} lines={diffLines} />
        </ChatToolCall>
        <ChatToolCall id="test-tool-summary" title="Cancelled lookup" kind="tool" status="cancelled" />
        <ChatToolCall id="test-tool-group" title="8 tool calls" status="error" group={{ errorCount: 1 }}>
          <div className="px-3 py-1">
            <ChatToolCall id="test-tool-group-success" title="git status --short" kind="git" status="completed" />
            <ChatToolCall id="test-tool-group-failure" title="bun run lint" kind="terminal" status="error" />
          </div>
        </ChatToolCall>
        <ChatToolCall id="test-tool-group-zero" title="8 tool calls" group={{ errorCount: 0 }} />
        <ChatToolCall id="test-tool-group-unknown" title="8 tool calls" status="error" group={{}} />
        <ChatToolCall id="test-tool-group-plural" title="8 tool calls" status="error" group={{ errorCount: 3 }} />
        <ChatToolCall id="test-tool-group-complete" title="8 tool calls" group={{}} />
        <ChatToolCall id="test-tool-agent" title={agentTitle} kind="agent" status={agentStatus} description="Started by Design System">
          <p id="test-agent-result" className="p-3 text-sm">{agentResult}</p>
        </ChatToolCall>
        <ChatToolCall
          id="test-tool-agent-link"
          title={agentTitle}
          kind="agent"
          status={agentStatus}
          href="#agent-chat-destination"
          linkProps={{ title: "Open the layout reviewer conversation", onClick: () => { state.agentLinkActivations++; } }}
        >
          <p id="test-agent-link-result" className="p-3 text-sm">{agentResult}</p>
        </ChatToolCall>
      </section>
      <section id="agent-chat-destination" aria-labelledby="test-agent-chat-heading">
        <h2 id="test-agent-chat-heading">Layout reviewer conversation</h2>
        <ChatThread label="Linked agent conversation" autoScroll={false}>
          <ChatMessage from="user"><ChatMessageContent>Check the compact chat layout.</ChatMessageContent></ChatMessage>
          <ChatMessage from="assistant"><ChatMessageContent>{agentResult}</ChatMessageContent></ChatMessage>
        </ChatThread>
      </section>
    </main>
  );
}

export function mount() {
  const gallery = document.getElementById("root");
  if (gallery) gallery.style.display = "none";
  const host = document.createElement("div");
  host.id = "chat-browser-fixture";
  document.body.appendChild(host);
  Object.assign(window, { __chatTest: state });
  createRoot(host).render(<Fixture />);
}
