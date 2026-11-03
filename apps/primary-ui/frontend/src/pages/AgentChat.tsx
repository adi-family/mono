import { useEffect } from "react";
import { ArrowLeft } from "lucide-react";
import { Link } from "react-router-dom";
import { ChatMarkdown } from "../components/ChatMarkdown";

import {
  ChatMessage,
  ChatMessageContent,
  ChatMessageMeta,
  ChatThread,
} from "@design-system/ui";

export function AgentChat() {
  useEffect(() => {
    document.title = "Layout reviewer — Design System";
  }, []);

  return (
    <main aria-labelledby="agent-chat-heading" data-demo="agent-conversation" className="mx-auto flex w-full max-w-3xl min-w-0 flex-col gap-4">
      <Link
        to="/library#chat-thread-example"
        data-slot="agent-chat-back"
        aria-label="Back to design conversation"
        className="text-muted-foreground hover:text-foreground focus-visible:ring-ring flex w-fit items-center gap-1.5 rounded-sm text-sm outline-none focus-visible:ring-2"
      >
        <ArrowLeft aria-hidden="true" className="size-3.5" />
        Design conversation
      </Link>
      <div>
        <h2 id="agent-chat-heading" className="text-base font-medium">Layout reviewer</h2>
        <p className="text-muted-foreground mt-1 text-sm">Example agent conversation</p>
      </div>
      <ChatThread
        label="Layout reviewer conversation"
        autoScroll={false}
        aria-live="off"
        className="h-[32rem] min-w-0 flex-none rounded-xl border"
      >
        <ChatMessage from="user">
          <div className="min-w-0">
            <ChatMessageMeta>Design assistant</ChatMessageMeta>
            <ChatMessageContent className="whitespace-normal"><ChatMarkdown>Check the chat spacing and scan the layout on narrow screens. Review keyboard navigation through the conversation and tool calls.</ChatMarkdown></ChatMessageContent>
          </div>
        </ChatMessage>
        <ChatMessage from="assistant">
          <div className="min-w-0">
            <ChatMessageMeta>Layout reviewer</ChatMessageMeta>
            <ChatMessageContent className="whitespace-normal"><ChatMarkdown>{"The message spacing is consistent and the conversation fits mobile screens.\n\nKeyboard focus reaches the tool calls and composer in order. Expanded details stay within the conversation width."}</ChatMarkdown></ChatMessageContent>
          </div>
        </ChatMessage>
      </ChatThread>
    </main>
  );
}
