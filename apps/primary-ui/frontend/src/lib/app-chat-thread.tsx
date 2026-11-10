import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { partialMatchKey, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  answerQuestion, cancelAwait, cancelQueuedMessage, getConversation, snapshotPageSize, stopConversation,
  type AgentPeek, type AgentTurn,
} from "./app-chat";
import { errorText, useScopedErrors } from "./app-chat-errors";
import { busyThreadPollMs, chatQueryKeys, idleThreadPollMs, invalidSourceMessage } from "./app-chat-queries";
import { useChatSelection } from "./app-chat-selection";
import { hasSource } from "./app-chat-sources";
import { reportEvent } from "./event-log";

function useThreadState() {
  const { enabled, invalidSource, sources, activeSource, agent, runId, active, selectionKey } = useChatSelection();
  const client = useQueryClient();
  const [limits, setLimits] = useState<Record<string, number>>({});
  const [stopping, setStopping] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState<readonly string[]>([]);
  const queueWrites = useRef<Promise<void>>(Promise.resolve());
  const actionErrors = useScopedErrors();

  const limit = limits[selectionKey] ?? snapshotPageSize;
  const ref = { agent, runId, source: activeSource };
  const snapshotKey = chatQueryKeys.snapshot(ref, limit);
  const threadKey = chatQueryKeys.thread(ref);
  const snapshotQuery = useQuery({
    queryKey: snapshotKey,
    queryFn: ({ signal }) => getConversation(ref, signal, { limit }),
    enabled: enabled && !invalidSource && active !== null && hasSource(sources, activeSource),
    retry: false,
    placeholderData: (previous, previousQuery) => previousQuery && partialMatchKey(previousQuery.queryKey, threadKey) ? previous : undefined,
    refetchInterval: (query) => query.state.data?.running || query.state.data?.turns.some((turn) => turn.pending || turn.queued) ? busyThreadPollMs : idleThreadPollMs,
  });
  const snapshot = active && !invalidSource ? snapshotQuery.data : undefined;

  // A transcript refresh that fails is logged, not shown over a transcript that is already there.
  // The error stays until a poll succeeds, so steady failure reports once per outage.
  const threadError = active && !invalidSource && snapshotQuery.error ? errorText(snapshotQuery.error) : null;
  useEffect(() => {
    if (threadError) reportEvent({ level: "error", area: "Chat", source: activeSource, message: threadError });
  }, [threadError, selectionKey, activeSource]);

  /** A mutation's response holds only the first page, so it must not replace a longer one. */
  function adoptResponse(response: AgentPeek) {
    if (limit <= snapshotPageSize) client.setQueryData(snapshotKey, response);
  }

  async function refresh() {
    actionErrors.clear();
    await client.invalidateQueries({ queryKey: chatQueryKeys.all });
  }

  async function stopResponse() {
    if (!active || stopping === selectionKey) return;
    const key = selectionKey;
    setStopping(key);
    actionErrors.clear();
    try {
      await stopConversation(active);
      await refresh();
    } catch (error) {
      actionErrors.report(key, errorText(error));
    } finally {
      setStopping((current) => current === key ? null : current);
    }
  }

  async function submitAnswer(ask: string, replies: string[]) {
    if (!active) throw new Error("Open a conversation first.");
    adoptResponse(await answerQuestion(active, ask, replies));
    void refresh();
  }

  const queuedToken = (turn: AgentTurn) => `${selectionKey}:queued:${turn.seq}`;
  const awaitToken = (id: string) => `${selectionKey}:await:${id}`;
  const settleCancel = (token: string) => setCancelling((current) => current.filter((item) => item !== token));

  /** `position` is the message's place among the snapshot's queued turns. */
  function cancelQueued(turn: AgentTurn, position: number) {
    const token = queuedToken(turn);
    if (!active || cancelling.includes(token)) return;
    const [target, key, adopt] = [active, selectionKey, adoptResponse];
    setCancelling((current) => [...current, token]);
    actionErrors.dismiss(key);
    // One at a time: each cancellation moves the positions the next one is addressed by.
    queueWrites.current = queueWrites.current.then(async () => {
      try {
        adopt(await cancelQueuedMessage(target, turn, position));
        void client.invalidateQueries({ queryKey: chatQueryKeys.thread(target) });
      } catch (error) {
        actionErrors.report(key, errorText(error));
      } finally {
        settleCancel(token);
      }
    });
  }

  async function cancelWaiting(id: string) {
    const token = awaitToken(id);
    if (!active || cancelling.includes(token)) return;
    const [target, key, cacheKey] = [active, selectionKey, snapshotKey];
    setCancelling((current) => [...current, token]);
    actionErrors.dismiss(key);
    try {
      const awaits = await cancelAwait(target, id);
      client.setQueryData<AgentPeek>(cacheKey, (current) => current && { ...current, awaits });
    } catch (error) {
      // An await that fired or was dropped elsewhere is refused; say that rather than relay the refusal.
      const fresh = await getConversation(target).catch(() => undefined);
      actionErrors.report(key, fresh && !fresh.awaits.some((item) => item.id === id)
        ? "That task is no longer waiting: it finished or was already cancelled."
        : errorText(error));
    } finally {
      settleCancel(token);
      void client.invalidateQueries({ queryKey: chatQueryKeys.all });
    }
  }

  function conversationError() {
    if (invalidSource) return invalidSourceMessage;
    return threadError;
  }

  return {
    snapshot, adoptResponse, refresh, stopResponse, submitAnswer, cancelQueued, cancelWaiting,
    isCancellingQueued: (turn: AgentTurn) => cancelling.includes(queuedToken(turn)),
    cancellingWaits: (snapshot?.awaits ?? []).flatMap((item) => cancelling.includes(awaitToken(item.id)) ? [item.id] : []),
    isLoadingConversation: active !== null && !invalidSource && snapshotQuery.isPending,
    conversationError: conversationError(),
    actionError: actionErrors.message(selectionKey),
    isStopping: stopping === selectionKey,
    loadEarlier: () => setLimits((current) => ({ ...current, [selectionKey]: limit + snapshotPageSize })),
    isLoadingEarlier: snapshotQuery.isFetching && snapshotQuery.isPlaceholderData,
  };
}

const ChatThreadContext = createContext<ReturnType<typeof useThreadState> | null>(null);

export function ChatThreadProvider({ children }: { children: ReactNode }) {
  return <ChatThreadContext.Provider value={useThreadState()}>{children}</ChatThreadContext.Provider>;
}

/** The open conversation's transcript and the actions that act on it. */
export function useChatThread() {
  const value = useContext(ChatThreadContext);
  if (!value) throw new Error("useChatThread must be inside AppChatProvider.");
  return value;
}
