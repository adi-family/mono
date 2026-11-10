import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";

import {
  getConversationIndex, hideConversation, renameConversation, starConversation, type Conversation, type ConversationIndex, type ConversationRef,
} from "./app-chat";
import { errorText } from "./app-chat-errors";
import { buildSourceRows, clearOverlayField, indexRows, patchIndex, previewText, rememberRows, type ChatOverlay, type ChatOverlays } from "./app-chat-model";
import { chatQueryKeys, indexPageSize, indexPollMs, projectPageSize, queryOutcomes } from "./app-chat-queries";
import { useChatSelection } from "./app-chat-selection";
import { conversationKey, sourceKey } from "./app-chat-sources";
import { useChatThread } from "./app-chat-thread";
import { reportEvent } from "./event-log";

interface ChatEdit<F extends keyof ChatOverlay> {
  ref: ConversationRef;
  field: F;
  value: NonNullable<ChatOverlay[F]>;
  write: () => Promise<void>;
  failure: string;
}

function useListState() {
  const { enabled, sourceList, filter, group, active, agentsBySource, agentResults, activeAgentsPending, isCurrentSelection, newConversationOn } = useChatSelection();
  const { snapshot } = useChatThread();
  const client = useQueryClient();
  const [indexLimits, setIndexLimits] = useState<Record<string, number>>({});
  const [projectMore, setProjectMore] = useState<Record<string, Record<string, number>>>({});
  const [overlays, setOverlays] = useState<ChatOverlays>({});
  const [mutationError, setMutationError] = useState<string | null>(null);
  const [remembered, setRemembered] = useState<ReadonlyMap<string, Conversation>>(() => new Map());
  const pendingEdits = useRef(new Set<string>());

  const indexResults = useQueries({
    queries: sourceList.map((source) => {
      const key = sourceKey(source);
      const limit = indexLimits[key] ?? indexPageSize;
      return {
        queryKey: chatQueryKeys.index(source, filter, group, limit, projectMore[key] ?? {}),
        queryFn: ({ signal }: { signal: AbortSignal }) => getConversationIndex(signal, source, {
          filter, limit,
          ...(group === "project" ? { group: "project" as const, groupLimit: projectPageSize, groupMore: projectMore[key] } : {}),
        }),
        enabled, refetchInterval: indexPollMs, retry: false,
      };
    }),
    combine: queryOutcomes<ConversationIndex>,
  });

  useEffect(() => {
    const rows = indexResults.flatMap((result) => indexRows(result.data));
    setRemembered((current) => rememberRows(current, rows));
  }, [indexResults]);

  const sourceStates = sourceList.map((source, position) => {
    const result = indexResults[position];
    const error = result.error ?? agentResults[position]?.error;
    return {
      ...buildSourceRows({ source, index: result.data, agents: agentsBySource[position] ?? [], overlays, filter, group, active, snapshot, remembered }),
      isLoading: result.isPending,
      error: error ? errorText(error) : null,
    };
  });
  const conversations = sourceStates.flatMap((state) => state.conversations).sort((a, b) => b.lastActivity - a.lastActivity);

  // A machine's polls fail in the background, so a failure goes to the event log rather than
  // the page: once when it starts, and a note when the machine answers again. The signature is
  // a string so that steady failure across polls reports nothing new.
  const errorSignature = JSON.stringify(sourceStates.map((state) => [sourceKey(state.source), state.error]));
  const previousErrors = useRef<ReadonlyMap<string, string | null>>(new Map());
  useEffect(() => {
    const entries = JSON.parse(errorSignature) as [string, string | null][];
    for (const [key, error] of entries) {
      const before = previousErrors.current.get(key) ?? null;
      if (error && error !== before) reportEvent({ level: "error", area: "Chats", source: key || undefined, message: error });
      else if (!error && before) reportEvent({ level: "info", area: "Chats", source: key || undefined, message: "Responding again." });
    }
    previousErrors.current = new Map(entries);
  }, [errorSignature]);

  function loadMore(source?: string, project?: string) {
    const key = sourceKey(source);
    if (project === undefined) {
      setIndexLimits((current) => ({ ...current, [key]: (current[key] ?? indexPageSize) + indexPageSize }));
      return;
    }
    setProjectMore((current) => ({ ...current, [key]: { ...current[key], [project]: (current[key]?.[project] ?? 0) + projectPageSize } }));
  }

  // Keep each field overlaid through the write and subsequent reads. Polling cannot
  // flash stale values, and rolling back one field preserves other pending edits.
  async function editChat<F extends keyof ChatOverlay>({ ref, field, value, write, failure }: ChatEdit<F>) {
    const key = conversationKey(ref);
    const editKey = `${key}:${field}`;
    if (pendingEdits.current.has(editKey)) return;
    pendingEdits.current.add(editKey);
    const overlay = { [field]: value } as ChatOverlay;
    setMutationError(null);
    setOverlays((current) => ({ ...current, [key]: { ...current[key], ...overlay } }));
    try {
      await write();
      if (overlay.hidden && isCurrentSelection(key)) newConversationOn(ref.source, ref.agent);
      // Update inactive filter/group caches too. A failed follow-up read must not
      // roll back a write the server has already accepted.
      client.setQueriesData<ConversationIndex>({ queryKey: chatQueryKeys.allIndexes }, (index) => index && patchIndex(index, key, overlay));
      await client.invalidateQueries({ queryKey: chatQueryKeys.allIndexes });
    } catch (error) {
      setMutationError(`${failure} ${errorText(error)}`);
      reportEvent({ level: "error", area: "Chats", source: ref.source, message: `${failure} ${errorText(error)}` });
    } finally {
      setOverlays((current) => clearOverlayField(current, key, field));
      pendingEdits.current.delete(editKey);
    }
  }

  function renameChat(ref: ConversationRef, title: string) {
    const defaultTitle = previewText(remembered.get(conversationKey(ref))?.message) || "Untitled chat";
    return editChat({ ref, field: "title", value: title.trim() || defaultTitle, write: () => renameConversation(ref, title), failure: "Could not rename this chat." });
  }

  function hideChat(ref: ConversationRef) {
    return editChat({ ref, field: "hidden", value: true, write: () => hideConversation(ref), failure: "Could not hide this chat." });
  }

  function starChat(ref: ConversationRef, starred: boolean) {
    return editChat({ ref, field: "starred", value: starred, write: () => starConversation(ref, starred), failure: "Could not update the chat star." });
  }

  return {
    sourceStates, conversations, loadMore, renameChat, hideChat, starChat,
    isLoading: activeAgentsPending || indexResults.some((result) => result.isPending),
    /** Whether any selected machine is failing to answer; the messages themselves are in the event log. */
    indexFailing: sourceStates.some((state) => state.error),
    mutationError,
    dismissMutationError: () => setMutationError(null),
  };
}

const ChatListContext = createContext<ReturnType<typeof useListState> | null>(null);

export function ChatListProvider({ children }: { children: ReactNode }) {
  return <ChatListContext.Provider value={useListState()}>{children}</ChatListContext.Provider>;
}

/** The sidebar's rows and the edits that act on any of them. */
export function useChatList() {
  const value = useContext(ChatListContext);
  if (!value) throw new Error("useChatList must be inside AppChatProvider.");
  return value;
}
