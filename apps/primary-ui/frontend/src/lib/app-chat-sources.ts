import type { ConversationRef } from "./app-chat";

export interface ChatSources { local: boolean; nodes: string[] }
export type ChatFilter = "all" | "starred" | "mine" | "me";
export type ChatGroup = "flat" | "machine" | "project";
export const localSource = "";
const validNode = (value: unknown): value is string => typeof value === "string" && /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(value);

export function normalizeSources(value: unknown): ChatSources {
  if (!value || typeof value !== "object" || Array.isArray(value)) return { local: true, nodes: [] };
  const input = value as Record<string, unknown>;
  if ((input.local !== undefined && typeof input.local !== "boolean") || (input.nodes !== undefined && (!Array.isArray(input.nodes) || !input.nodes.every(validNode)))) return { local: true, nodes: [] };
  const nodes = [...new Set((input.nodes ?? []) as string[])].sort();
  return { local: input.local !== false || nodes.length === 0, nodes };
}

export function readSources(): ChatSources {
  try { return normalizeSources(JSON.parse(localStorage.getItem("adi-session-sources") ?? "null")); }
  catch { return { local: true, nodes: [] }; }
}
export function readGroup(): ChatGroup {
  try {
    const value: unknown = JSON.parse(localStorage.getItem("adi-session-group") ?? "null");
    return value === "project" || value === "flat" ? value : value === "activity" ? "flat" : "machine";
  } catch { return "machine"; }
}
export function savePreference(key: string, value: unknown) {
  try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* In-memory controls still work in private mode. */ }
}
export function sourceName(source?: string) { return source || "This machine"; }
export function conversationKey(ref: ConversationRef) { return JSON.stringify([ref.source ?? localSource, ref.agent, ref.runId]); }
export function sameConversation(a: ConversationRef, b: ConversationRef | null) {
  return b !== null && conversationKey(a) === conversationKey(b);
}
export function selectedSources(sources: ChatSources): (string | undefined)[] {
  return [...(sources.local ? [undefined] : []), ...sources.nodes];
}
export function hasSource(sources: ChatSources, source?: string) { return source ? sources.nodes.includes(source) : sources.local; }
export function routeNode(value: string | null): string | undefined { return validNode(value) ? value : undefined; }
