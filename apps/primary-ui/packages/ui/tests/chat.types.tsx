import type * as React from "react";

import {
  ChatComposer,
  ChatMessage,
  ChatMessageContent,
  ChatThread,
  type ChatComposerProps,
  type ChatThreadDirection,
} from "../src/components";

const draftProps = {
  value: "Hello",
  onValueChange: (_value: string) => {},
  onSend: (_message: string) => {},
};

function ComposerWrapper(props: React.ComponentProps<typeof ChatComposer>) {
  return <ChatComposer {...props} />;
}

// Compile-only contracts: an unused @ts-expect-error fails the library typecheck.
export function acceptedChatExamples(disabled: boolean) {
  const formRef: React.RefObject<HTMLFormElement | null> = { current: null };
  const threadRef: React.RefObject<HTMLDivElement | null> = { current: null };
  const articleRef: React.RefObject<HTMLElement | null> = { current: null };
  const props: ChatComposerProps = {
    ...draftProps,
    disabled,
    disabledReason: "Messaging is unavailable in this workspace.",
  };
  const direction: ChatThreadDirection = "bottom-to-top";

  return (
    <>
      <ChatComposer {...draftProps} ref={formRef} />
      <ChatComposer {...props} />
      <ComposerWrapper {...props} />
      <ChatComposer {...draftProps} onSend={async (_message) => {}} />
      <ChatComposer
        {...draftProps}
        textareaProps={{
          id: "message",
          name: "message",
          maxLength: 2000,
          "aria-describedby": "message-help",
          onKeyDown: (event) => { if (event.key === "Escape") event.preventDefault(); },
        }}
      />
      <ChatThread ref={threadRef} autoScroll={false} aria-live="off">
        <ChatMessage from="assistant" ref={articleRef} aria-label="Assistant">
          <ChatMessageContent><strong>Hello</strong></ChatMessageContent>
        </ChatMessage>
        <ChatMessage from="user" />
        <ChatMessage from="system" />
      </ChatThread>
      <ChatThread direction="top-to-bottom" />
      <ChatThread direction={direction} />
    </>
  );
}

export function rejectedChatExamples(disabled: boolean) {
  // @ts-expect-error A composer needs a controlled draft value.
  <ChatComposer onValueChange={draftProps.onValueChange} onSend={draftProps.onSend} />;
  // @ts-expect-error A controlled composer must report draft changes.
  <ChatComposer value="Hello" onSend={draftProps.onSend} />;
  // @ts-expect-error A composer must have a send handler.
  <ChatComposer value="Hello" onValueChange={draftProps.onValueChange} />;
  // @ts-expect-error Uncontrolled defaults cannot replace the controlled draft contract.
  <ChatComposer defaultValue="Hello" onValueChange={draftProps.onValueChange} onSend={draftProps.onSend} />;
  // @ts-expect-error The send lifecycle cannot be bypassed through a native submit handler.
  <ChatComposer {...draftProps} onSubmit={() => {}} />;

  // @ts-expect-error Disabled composers need an explanation.
  <ChatComposer {...draftProps} disabled />;
  // @ts-expect-error A potentially disabled composer also needs an explanation.
  <ChatComposer {...draftProps} disabled={disabled} />;
  // @ts-expect-error Wrappers retain the disabled explanation contract.
  <ComposerWrapper {...draftProps} disabled />;
  // @ts-expect-error React's inferred props retain the disabled explanation contract.
  const invalidDisabled: React.ComponentProps<typeof ChatComposer> = { ...draftProps, disabled: true };

  // @ts-expect-error Textarea values belong to the composer.
  <ChatComposer {...draftProps} textareaProps={{ value: "Other draft" }} />;
  // @ts-expect-error Native default values cannot make the controlled textarea uncontrolled.
  <ChatComposer {...draftProps} textareaProps={{ defaultValue: "Other draft" }} />;
  // @ts-expect-error Textarea changes must pass through onValueChange.
  <ChatComposer {...draftProps} textareaProps={{ onChange: () => {} }} />;
  // @ts-expect-error Native disabled cannot bypass the composer explanation contract.
  <ChatComposer {...draftProps} textareaProps={{ disabled: true }} />;
  // @ts-expect-error Native children cannot replace the controlled textarea's contents.
  <ChatComposer {...draftProps} textareaProps={{ children: "Other draft" }} />;
  // @ts-expect-error The textarea must submit its own composer, not an external form.
  <ChatComposer {...draftProps} textareaProps={{ form: "another-form" }} />;

  // @ts-expect-error Every message must specify its speaker role.
  <ChatMessage><ChatMessageContent>Hello</ChatMessageContent></ChatMessage>;
  // @ts-expect-error Speaker roles are limited to the supported message variants.
  <ChatMessage from="tool" />;

  // @ts-expect-error Thread direction is one of the two chronological layouts.
  <ChatThread direction="left-to-right" />;
  // @ts-expect-error The exported direction type matches the thread's supported layouts.
  const invalidDirection: ChatThreadDirection = "reverse";

  return { invalidDisabled, invalidDirection };
}
