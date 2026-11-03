import type * as React from "react";

import {
  ChatToolCall,
  ChatToolDiff,
  ChatToolOutput,
  type ChatToolCallKind,
  type ChatToolCallProps,
  type ChatToolCallStatus,
  type ChatToolDiffLine,
} from "../src/components";

function ToolWrapper(props: React.ComponentProps<typeof ChatToolCall>) {
  return <ChatToolCall {...props} />;
}

export function acceptedToolExamples() {
  const rootRef: React.RefObject<HTMLDivElement | null> = { current: null };
  const outputRef: React.RefObject<HTMLPreElement | null> = { current: null };
  const kind: ChatToolCallKind = "git";
  const status: ChatToolCallStatus = "running";
  const lines: readonly ChatToolDiffLine[] = [
    { type: "context", content: "function render() {", oldLineNumber: 1, newLineNumber: 1 },
    { type: "deletion", content: "  return null;", oldLineNumber: 2 },
    { type: "addition", content: "  return <Chat />;", newLineNumber: 2 },
  ];
  const props: ChatToolCallProps = { title: "Run tests", kind, status, defaultOpen: true };

  return (
    <>
      <ChatToolCall {...props} ref={rootRef} aria-label="Test execution">
        <ChatToolOutput ref={outputRef} label="Test results" tabIndex={0}>All checks passed</ChatToolOutput>
      </ChatToolCall>
      <ToolWrapper {...props} />
      <ChatToolCall title="Edit file" kind="edit" open onOpenChange={(_open: boolean) => {}} meta={<span>1 file</span>}>
        <ChatToolDiff ref={rootRef} file="chat.tsx" lines={lines} />
      </ChatToolCall>
      <ChatToolCall title="Generic lookup" />
      <ChatToolCall title="Layout reviewer · Check chat layout" kind="agent" description="Started by Design System">
        <p>The compact activity row fits the conversation.</p>
      </ChatToolCall>
      <ToolWrapper
        title="Layout reviewer · Check chat layout"
        kind="agent"
        href="/agents/layout-reviewer"
        linkProps={{
          target: "_blank",
          rel: "noopener",
          onClick: (event) => { const anchor: HTMLAnchorElement = event.currentTarget; void anchor.href; },
        }}
      />
      <ChatToolCall title="Failed command" status="error" />
      <ChatToolCall title="Cancelled command" status="cancelled" />
      <ChatToolCall title="Pending command" status="pending" />
      <ToolWrapper title="8 tool calls" status="error" group={{ errorCount: 1 }}>
        <ChatToolCall title="Failed child command" status="error" />
      </ToolWrapper>
      <ChatToolCall title="Completed calls" group={{ errorCount: 0 }} />
      <ChatToolCall title="Calls with unknown error count" status="error" group={{}} />
    </>
  );
}

export function rejectedToolExamples() {
  // @ts-expect-error Tool summaries require a title.
  <ChatToolCall status="running" />;
  // @ts-expect-error Wrappers retain the required summary title.
  <ToolWrapper kind="terminal" />;
  // @ts-expect-error Status is limited to execution lifecycle states.
  <ChatToolCall title="Run tests" status="success" />;
  // @ts-expect-error Kind is limited to the supported visual variants.
  <ChatToolCall title="Run tests" kind="shell" />;
  // @ts-expect-error A group uses an options object, allowing error counts to be omitted.
  <ChatToolCall title="8 tool calls" group />;
  // @ts-expect-error Group error counts are numeric.
  <ToolWrapper title="8 tool calls" group={{ errorCount: "1" }} />;
  // @ts-expect-error The destination belongs to the row, so link props cannot override it.
  <ChatToolCall title="Agent chat" href="/agents/reviewer" linkProps={{ href: "/another-chat" }} />;
  // @ts-expect-error The row summary provides the link's children.
  <ChatToolCall title="Agent chat" href="/agents/reviewer" linkProps={{ children: "Another title" }} />;
  // @ts-expect-error Output is a literal string, so a renderer cannot silently interpret markup.
  <ChatToolOutput><strong>Output</strong></ChatToolOutput>;
  // @ts-expect-error A diff needs a file label.
  <ChatToolDiff lines={[]} />;
  // @ts-expect-error A diff needs explicit lines.
  <ChatToolDiff file="chat.tsx" />;
  // @ts-expect-error Diff lines have one of three supported change types.
  <ChatToolDiff file="chat.tsx" lines={[{ type: "modified", content: "render()" }]} />;
}
