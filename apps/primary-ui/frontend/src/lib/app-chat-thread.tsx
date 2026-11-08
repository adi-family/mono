import { createContext, useContext, useState, type ReactNode } from "react";
import { partialMatchKey, useQuery, useQueryClient } from "@tanstack/react-query";

import { answerQuestion, getConversation, snapshotPageSize, stopConversation, type AgentPeek } from "./app-chat";
import { errorText, useScopedErrors } from "./app-chat-errors";
import { busyThreadPollMs, chatQueryKeys, idleThreadPollMs, invalidSourceMessage } from "./app-chat-queries";
import { useChatSelection } from "./app-chat-selection";
import { hasSource } from "./app-chat-sources";

function useThreadState() {
  const { enabled, invalidSource, sources, activeSource, agent, runId, active, selectionKey } = useChatSelection();
  const client = useQueryClient();
  const [limits, setLimits] = useState<Record<string, number>>({});
  const [stopping, setStopping] = useState<string | null>(null);
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

  function conversationError() {
    if (invalidSource) return invalidSourceMessage;
    return active && snapshotQuery.error ? errorText(snapshotQuery.error) : null;
  }

  return {
    snapshot, adoptResponse, refresh, stopResponse, submitAnswer,
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
