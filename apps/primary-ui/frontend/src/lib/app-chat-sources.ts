import {
  jsonStorageCodec,
  type LocalStorageCodec,
  readLocalStorage,
  writeLocalStorage,
} from "@design-system/ui";

import type { ConversationRef } from "./app-chat";

export interface ChatSources {
  local: boolean;
  nodes: string[];
}
export type ChatFilter = "all" | "starred" | "mine" | "me";
export type ChatGroup = "flat" | "machine" | "project";
export const localSource = "";
export const sourcesPreferenceKey = "adi-session-sources";
export const groupPreferenceKey = "adi-session-group";
export const defaultSources: ChatSources = { local: true, nodes: [] };
const validNode = (value: unknown): value is string =>
  typeof value === "string" && /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(value);

export function normalizeSources(value: unknown): ChatSources {
  if (!value || typeof value !== "object" || Array.isArray(value))
    return { local: true, nodes: [] };
  const input = value as Record<string, unknown>;
  if (
    (input.local !== undefined && typeof input.local !== "boolean") ||
    (input.nodes !== undefined && (!Array.isArray(input.nodes) || !input.nodes.every(validNode)))
  )
    return { local: true, nodes: [] };
  const nodes = [...new Set((input.nodes ?? []) as string[])].sort();
  return { local: input.local !== false || nodes.length === 0, nodes };
}

export const sourcesStorageCodec: LocalStorageCodec<ChatSources> = {
  ...jsonStorageCodec,
  parse: (stored) => normalizeSources(jsonStorageCodec.parse(stored)),
};

export const groupStorageCodec: LocalStorageCodec<ChatGroup> = {
  ...jsonStorageCodec,
  parse: (stored) => {
    const value = jsonStorageCodec.parse(stored);
    return value === "project" || value === "flat"
      ? value
      : value === "activity"
        ? "flat"
        : undefined;
  },
};

export function readSources(): ChatSources {
  return readLocalStorage(sourcesPreferenceKey, defaultSources, sourcesStorageCodec);
}
export function readGroup(): ChatGroup {
  return readLocalStorage(groupPreferenceKey, "machine", groupStorageCodec);
}
export function savePreference(key: string, value: unknown) {
  writeLocalStorage(key, value);
}
/** The form of a source used in map keys and query keys; `undefined` means this machine. */
export function sourceKey(source?: string) {
  return source ?? localSource;
}
export function sourceName(source?: string) {
  return source || "This machine";
}
export function conversationKey(ref: ConversationRef) {
  return JSON.stringify([sourceKey(ref.source), ref.agent, ref.runId]);
}
export function sameConversation(a: ConversationRef, b: ConversationRef | null) {
  return b !== null && conversationKey(a) === conversationKey(b);
}
export function selectedSources(sources: ChatSources): (string | undefined)[] {
  return [...(sources.local ? [undefined] : []), ...sources.nodes];
}
export function hasSource(sources: ChatSources, source?: string) {
  return source ? sources.nodes.includes(source) : sources.local;
}
export function withSource(sources: ChatSources, source?: string): ChatSources {
  return normalizeSources(
    source ? { ...sources, nodes: [...sources.nodes, source] } : { ...sources, local: true },
  );
}
export function toggledSource(sources: ChatSources, source?: string): ChatSources {
  if (!source) return normalizeSources({ ...sources, local: !sources.local });
  const nodes = sources.nodes.includes(source)
    ? sources.nodes.filter((node) => node !== source)
    : [...sources.nodes, source];
  return normalizeSources({ ...sources, nodes });
}
export function routeNode(value: string | null): string | undefined {
  return validNode(value) ? value : undefined;
}

export type LastLaunchedAgent = { agent: string; source?: string };
export const lastLaunchedAgentKey = "adi.last-launched-agent.v1";
export const lastLaunchedAgentCodec: LocalStorageCodec<LastLaunchedAgent | undefined> = {
  ...jsonStorageCodec,
  parse: (stored) => {
    const value = jsonStorageCodec.parse(stored) as
      { agent?: unknown; source?: unknown } | null | undefined;
    if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
    if (typeof value.agent !== "string" || !value.agent.trim()) return undefined;
    if (value.source !== undefined && !validNode(value.source)) return undefined;
    return { agent: value.agent, ...(value.source ? { source: value.source } : {}) };
  },
};
export function readLastLaunchedAgent(): LastLaunchedAgent | undefined {
  return readLocalStorage(lastLaunchedAgentKey, undefined, lastLaunchedAgentCodec);
}
