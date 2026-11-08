import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { ArrowLeft, ExternalLink, LoaderCircle, RefreshCw, Square } from "lucide-react";
import { Link } from "react-router-dom";
import { Button, ChatComposer, ChatQueuedMessage, ChatThread, ChatWaitingStatus } from "@design-system/ui";

import { ChatMicrophone } from "../components/ChatMicrophone";
import { NewChatSetup } from "../components/NewChatSetup";
import { ChatTitle } from "../components/ChatTitle";

import { AppChatAttachments, appChatTranscriptEntries } from "../components/AppChatTranscript";
import type { AgentAsk } from "../lib/app-chat";
import { useChatComposer } from "../lib/app-chat-composer";
import { useChatList } from "../lib/app-chat-list";
import { useChatSelection } from "../lib/app-chat-selection";
import { useChatThread } from "../lib/app-chat-thread";
import { sameConversation } from "../lib/app-chat-sources";
import { appConversationHref } from "../lib/app-chat-transcript";

function PendingQuestion({ ask, onAnswer }: {
  ask: AgentAsk;
  onAnswer: (id: string, replies: string[]) => Promise<void>;
}) {
  const id = useId();
  const [answers, setAnswers] = useState(() => ask.questions.map(() => ({ selected: [] as string[], text: "" })));
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const replies = answers.map(({ selected, text }) => [...selected, text.trim()].filter(Boolean).join(", "));
  const hasAnswer = replies.some((reply) => reply.length > 0);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!hasAnswer || inFlight.current) return;
    inFlight.current = true;
    setSending(true);
    setError(null);
    try {
      await onAnswer(ask.id, replies);
    } catch (error) {
      setError(error instanceof Error ? error.message : "Your answer could not be sent. Try again.");
    } finally {
      inFlight.current = false;
      setSending(false);
    }
  }

  return (
    <form data-slot="app-chat-question" className="min-w-0 space-y-4 rounded-xl border p-4" aria-label={ask.headline || "Agent question"} aria-busy={sending || undefined} onSubmit={submit}>
      {ask.headline && <p className="text-sm font-medium">{ask.headline}</p>}
      {ask.note && <p className="text-muted-foreground text-sm whitespace-pre-wrap [overflow-wrap:anywhere]">{ask.note}</p>}
      <p className="text-muted-foreground text-xs">Answer any questions you like. Choose an option, write your own answer, or combine both.</p>
      {ask.questions.map((question, index) => (
        <fieldset key={index} className="min-w-0 space-y-2" disabled={sending}>
          <legend className="mb-2 text-sm font-medium">
            {question.header && <span className="text-muted-foreground mr-2 text-xs">{question.header}</span>}
            {question.question || "Your answer"}
          </legend>
          {question.options.map((option) => (
            <label key={option.label} className="flex cursor-pointer items-start gap-2 rounded-md px-1 py-1 text-sm">
              <input
                className="accent-primary mt-1 shrink-0"
                type={question.multi_select ? "checkbox" : "radio"}
                name={`${id}-${index}`}
                onClick={() => {
                  if (!question.multi_select && answers[index]?.selected.includes(option.label)) {
                    setAnswers((current) => current.map((answer, position) => position === index ? { ...answer, selected: [] } : answer));
                  }
                }}
                checked={answers[index]?.selected.includes(option.label) ?? false}
                onChange={(event) => setAnswers((current) => current.map((answer, position) => position !== index ? answer : {
                  ...answer,
                  selected: question.multi_select
                    ? event.target.checked ? [...answer.selected, option.label] : answer.selected.filter((label) => label !== option.label)
                    : event.target.checked ? [option.label] : [],
                }))}
              />
              <span className="min-w-0 [overflow-wrap:anywhere]">
                <span>{option.label}</span>
                {option.description && <span className="text-muted-foreground mt-0.5 block text-xs leading-relaxed">{option.description}</span>}
              </span>
            </label>
          ))}
          <label className="block space-y-1.5 text-xs" htmlFor={`${id}-${index}-reply`}>
            <span className="text-muted-foreground">{question.options.length ? "Your answer or additional details" : "Your answer"}</span>
            <textarea
              data-slot="app-chat-answer-input"
              id={`${id}-${index}-reply`}
              rows={2}
              value={answers[index]?.text ?? ""}
              onChange={(event) => setAnswers((current) => current.map((answer, position) => position === index ? { ...answer, text: event.target.value } : answer))}
              className="border-input bg-background focus-visible:ring-ring/50 field-sizing-content max-h-36 min-h-16 w-full resize-none rounded-md border px-3 py-2 text-sm outline-none focus-visible:ring-2"
            />
          </label>
        </fieldset>
      ))}
      {error && <p role="alert" className="text-destructive text-sm">{error}</p>}
      <Button data-slot="app-chat-answer-submit" type="submit" size="sm" disabled={sending || !hasAnswer} disabledReason={sending ? "Your answer is being sent." : "Answer at least one question before sending."}>
        {sending && <LoaderCircle className="size-3.5 motion-safe:animate-spin" aria-hidden="true" />}
        Send answer
      </Button>
    </form>
  );
}

export function Interface() {
  const { agents, active, agent, selectionKey, activeSource, openConversation } = useChatSelection();
  const { snapshot, isLoadingConversation, conversationError, actionError, isStopping, refresh, stopResponse, submitAnswer, loadEarlier, isLoadingEarlier } = useChatThread();
  const { conversations, isLoading, indexError, renameChat, starChat } = useChatList();
  const { draft, setDraft, appendDraft, images, addImages, removeImage, imageError, sendMessage } = useChatComposer();
  const [voiceBusy, setVoiceBusy] = useState(false);
  const selected = agents.find((item) => item.name === agent);
  const conversation = active && conversations.find((item) => sameConversation(item, active));
  const title = active ? conversation?.title || "Conversation" : "New chat";
  const running = active !== null && (snapshot?.running ?? conversation?.running ?? false);
  const ask = snapshot?.pending_question;
  const loading = active ? isLoadingConversation && !snapshot : isLoading && !agents.length;
  const error = conversationError || (!active ? indexError : null);
  const disabledReason = !agent ? "Choose an agent to start a chat."
    : loading ? "The chat is loading."
      : active && !snapshot ? "Load this conversation before sending a message."
        : active && snapshot?.answerable === false ? "This conversation does not accept replies."
          : !active && selected?.caps?.answerable === false ? "Choose an agent that accepts chat messages."
          : !active && (!selected || selected.runnable === false) ? "This agent cannot start a new chat."
            : ask ? "Answer the agent’s question to continue." : "";

  useEffect(() => { document.title = `${title} — ADI`; }, [title]);

  function retry() { void refresh().catch(() => {}); }

  return (
    <main data-slot="app-chat" aria-label="ADI chat" className="flex h-full min-h-0 min-w-0 flex-1 flex-col">
      <header className="flex min-w-0 shrink-0 items-center justify-between gap-3 border-b px-3 py-2 sm:px-4">
        <Button asChild variant="ghost" size="icon" className="shrink-0 md:hidden">
          <Link to="/interface" aria-label="Back to chats"><ArrowLeft aria-hidden="true" className="size-5" /></Link>
        </Button>
        <div className="min-w-0 flex-1">
          <ChatTitle key={selectionKey} title={title} conversation={active} onRename={renameChat} starred={conversation?.starred === true} onStar={starChat} />
          {active && <p className="text-muted-foreground mt-0.5 truncate text-xs">{active.agent}{active.source ? ` · ${active.source}` : ""}</p>}

        </div>
        <div className="flex shrink-0 items-center gap-1">
          {running && <Button type="button" size="icon" variant="ghost" aria-label="Stop response" disabled={isStopping} disabledReason="The response is being stopped." onClick={() => { void stopResponse(); }}>
            {isStopping ? <LoaderCircle className="size-3.5 motion-safe:animate-spin" aria-hidden="true" /> : <Square className="size-3 fill-current" aria-hidden="true" />}
          </Button>}
          <Button type="button" size="icon" variant="ghost" aria-label="Refresh chat" disabled={isLoading} disabledReason="The chat is loading." onClick={retry}>
            <RefreshCw className={`size-3.5${isLoading ? " motion-safe:animate-spin" : ""}`} aria-hidden="true" />
          </Button>
          {active && <Button asChild size="icon" variant="ghost" className="hidden md:inline-flex">
            <a href={appConversationHref(active)} target="_blank" rel="noreferrer" aria-label="Open in ADI">
              <ExternalLink className="size-3.5" aria-hidden="true" />
            </a>
          </Button>}
        </div>
      </header>

      {error && (!loading || snapshot) && <div className="flex shrink-0 items-center gap-3 border-b px-4 py-2 text-xs sm:px-5">
        <p role="alert" className="text-destructive min-w-0 flex-1 [overflow-wrap:anywhere]">{error}</p>
        <Button type="button" size="sm" variant="ghost" onClick={retry}>Retry</Button>
      </div>}
      {actionError && <p role="alert" className="text-destructive shrink-0 px-4 py-2 text-xs sm:px-5">{actionError}</p>}

      {active && snapshot ? <ChatThread key={selectionKey} label="Chat messages" aria-live="off" className="mx-auto w-full max-w-4xl">
        {snapshot.total_turns > snapshot.turns.length && <div key="earlier" className="flex justify-center">
          <Button type="button" size="sm" variant="ghost" onClick={loadEarlier} disabled={isLoadingEarlier} disabledReason="Earlier messages are loading.">
            {isLoadingEarlier ? "Loading earlier messages…" : "Load earlier messages"}
          </Button>
        </div>}
        {appChatTranscriptEntries({ conversation: active, snapshot, conversations, onOpenConversation: openConversation })}
        {ask && <PendingQuestion key={`${selectionKey}:${ask.id}`} ask={ask} onAnswer={submitAnswer} />}
      </ChatThread> : <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 overflow-y-auto px-6 py-8 text-center">
        {loading ? <p role="status" className="text-muted-foreground text-sm">{active ? "Loading conversation…" : "Loading agents…"}</p>
          : active ? <p className="text-muted-foreground text-sm">This conversation could not be loaded.</p>
            : <NewChatSetup />}
      </div>}

      <div className="mx-auto w-full max-w-4xl shrink-0 px-3 pb-3 pt-2 sm:px-4 sm:pb-4">
        <ChatWaitingStatus className="mb-2" items={(snapshot?.awaits ?? []).map((item) => ({
          id: item.id,
          label: item.note.trim() || item.summary.trim() || "Waiting for a background task to finish.",
        }))} />
        {snapshot?.turns.some((turn) => turn.queued) && <div aria-label="Queued messages" className="mb-2 max-h-52 space-y-1 overflow-y-auto">
          {snapshot.turns.filter((turn) => turn.queued).map((turn) => <ChatQueuedMessage key={`${selectionKey}:${turn.seq}`} data-queued="true" text={turn.text} label={turn.mode === "asap" ? "Queued · ASAP" : "Queued"}>
            <AppChatAttachments turn={turn} source={activeSource} />
          </ChatQueuedMessage>)}
        </div>}
        {imageError && <p role="alert" className="text-destructive mb-2 text-xs">{imageError}</p>}
        <ChatComposer
          key={selectionKey}
          value={draft}
          onValueChange={setDraft}
          onSend={sendMessage}
          attachments={images}
          attachmentsBusy={images.some((image) => image.status === "uploading")}
          onFilesSelect={addImages}
          onAttachmentRemove={removeImage}
          disabled={Boolean(disabledReason) || voiceBusy}
          disabledReason={voiceBusy ? "Finish dictation before sending your message." : disabledReason || "The chat is unavailable."}
          placeholder={running ? "Send a follow-up…" : "Write a message…"}
          sendLabel={running ? "Queue message" : "Send message"}
        >
          <ChatMicrophone key={selectionKey} onText={appendDraft} disabledReason={disabledReason} onBusyChange={setVoiceBusy} />
        </ChatComposer>
      </div>
    </main>
  );
}
