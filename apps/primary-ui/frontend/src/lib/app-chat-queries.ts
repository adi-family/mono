import type { UseQueryResult } from "@tanstack/react-query";

import type { ConversationRef } from "./app-chat";
import { sourceKey, type ChatFilter, type ChatGroup } from "./app-chat-sources";

export const chatRoute = "/interface";
export const invalidSourceMessage = "This chat source is invalid. Choose a source from the sidebar.";

export const fleetPollMs = 15_000;
export const agentPollMs = 5_000;
export const indexPollMs = 5_000;
export const idleThreadPollMs = 5_000;
export const busyThreadPollMs = 1_000;

export const indexPageSize = 100;
export const projectPageSize = 3;

const chatRoot = ["app-chat"] as const;
const threadKey = (ref: ConversationRef) => [...chatRoot, "conversation", sourceKey(ref.source), ref.agent, ref.runId] as const;

export const chatQueryKeys = {
  all: chatRoot,
  fleet: [...chatRoot, "fleet"] as const,
  voice: [...chatRoot, "voice"] as const,
  agents: (source?: string) => [...chatRoot, "agents", sourceKey(source)] as const,
  allIndexes: [...chatRoot, "conversations"] as const,
  index: (source: string | undefined, filter: ChatFilter, group: ChatGroup, limit: number, projectMore: Record<string, number>) =>
    [...chatRoot, "conversations", sourceKey(source), filter, group, limit, projectMore] as const,
  thread: threadKey,
  snapshot: (ref: ConversationRef, limit: number) => [...threadKey(ref), limit] as const,
};

/**
 * Keeps only the fields the chat reads. The combined value is structurally shared,
 * so it stays referentially stable across background refetches that change nothing.
 */
export function queryOutcomes<T>(results: UseQueryResult<T>[]) {
  return results.map(({ data, error, isPending }) => ({ data, error, isPending }));
}
