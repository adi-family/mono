import type { Conversation, ConversationRef } from "./app-chat";
import { sameConversation } from "./app-chat-sources";

export const conversationFilters = [
  { value: "all", label: "All sessions" },
  { value: "starred", label: "Only starred" },
  { value: "mine", label: "Only started by any user" },
  { value: "me", label: "Only started by me" },
] as const;

export const conversationGroups = [
  { value: "flat", label: "Disabled" },
  { value: "machine", label: "Machine" },
  { value: "project", label: "Project" },
] as const;

export type ConversationFilter = typeof conversationFilters[number]["value"];
export type ConversationGroup = typeof conversationGroups[number]["value"];

/** The run index supplies Unix timestamps in milliseconds. */
export function conversationAge(lastActivity: number, now = Date.now()): string | null {
  if (!Number.isFinite(lastActivity) || lastActivity <= 0) return null;
  const seconds = Math.max(0, Math.floor((now - lastActivity) / 1000));
  if (seconds < 60) return "Just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  if (seconds < 604800) return `${Math.floor(seconds / 86400)}d ago`;
  if (seconds < 2592000) return `${Math.floor(seconds / 604800)}w ago`;
  if (seconds < 31536000) return `${Math.floor(seconds / 2592000)}mo ago`;
  return `${Math.floor(seconds / 31536000)}y ago`;
}

export interface SidebarSourceState {
  source?: string;
  conversations: Conversation[];
  total: number;
  loaded?: number;
  groups?: { project?: string; label: string; conversations: Conversation[]; total: number; loaded?: number }[];
  isLoading: boolean;
  error: string | null;
}

interface SidebarPage {
  source?: string;
  project?: string;
  total: number;
  loaded: number;
}

export interface ConversationSection {
  key: string;
  label?: string;
  conversations: Conversation[];
  pages: SidebarPage[];
}

/** Mirror the server over the open conversation, whose history is fetched separately. */
export function matchesConversationFilter(conversation: Conversation, filter: ConversationFilter, active: ConversationRef | null): boolean {
  if (sameConversation(conversation, active) || conversation.pendingQuestion || filter === "all") return true;
  if (filter === "starred") return conversation.agentStarred === true;
  const launcher = conversation.launchedBy ?? "";
  const human = launcher === "human" || launcher.startsWith("human:");
  if (!human) return false;
  if (filter === "mine") return true;
  const user = launcher === "human" ? null : launcher.slice("human:".length) || null;
  return user === (conversation.askingUser ?? null);
}

function activity(conversation: Conversation): number {
  if (conversation.pendingQuestion) return 0;
  if (conversation.running) return 1;
  if (conversation.awaiting) return 2;
  return conversation.starred ? 3 : 4;
}

function sorted(conversations: Conversation[]): Conversation[] {
  return [...conversations].sort((left, right) => activity(left) - activity(right) || right.lastActivity - left.lastActivity);
}

/** Project headings come from each source's index, including its own display names. */
export function conversationSections(
  conversations: Conversation[], sources: SidebarSourceState[], group: ConversationGroup, query: string,
): ConversationSection[] {
  const search = query.trim().toLocaleLowerCase();
  const projectFor = (conversation: Conversation) => sources.find((source) => source.source === conversation.source)?.groups?.find(
    (project) => project.conversations.some((item) => item.agent === conversation.agent),
  );
  const projectLabel = (conversation: Conversation) => projectFor(conversation)?.label ?? conversation.projectName ?? conversation.project ?? "No project";
  const matches = (conversation: Conversation) => [
    conversation.title, conversation.agent, conversation.source ?? "This machine", projectLabel(conversation),
  ].join(" ").toLocaleLowerCase().includes(search);
  const visible = sorted(conversations.filter(matches));
  const pages = sources.map((source) => ({ source: source.source, total: source.total, loaded: source.loaded ?? source.conversations.length }));

  if (group === "flat") {
    return [{ key: "all", conversations: visible, pages }];
  }
  if (group === "machine") {
    return sources.map((source) => ({
      key: JSON.stringify([source.source ?? ""]),
      label: source.source ?? "This machine",
      conversations: visible.filter((conversation) => conversation.source === source.source),
      pages: [{ source: source.source, total: source.total, loaded: source.loaded ?? source.conversations.length }],
    })).filter((section) => section.conversations.length > 0 || (!search && section.pages.some((page) => page.total > page.loaded)));
  }

  const sections = new Map<string, ConversationSection>();
  for (const source of sources) {
    for (const project of source.groups ?? []) {
      const section = sections.get(project.label) ?? { key: `project:${project.label}`, label: project.label, conversations: [], pages: [] };
      section.pages.push({ source: source.source, project: project.project ?? "~", total: project.total, loaded: project.loaded ?? project.conversations.length });
      sections.set(project.label, section);
    }
  }
  for (const conversation of visible) {
    const label = projectLabel(conversation);
    const section = sections.get(label) ?? { key: `project:${label}`, label, conversations: [], pages: [] };
    section.conversations.push(conversation);
    sections.set(label, section);
  }
  // Keep the server's source order, with the unfiled section last across the merged list.
  const unfiled = sections.get("No project");
  sections.delete("No project");
  if (unfiled) sections.set("No project", unfiled);
  return [...sections.values()].filter((section) => section.conversations.length > 0 || (!search && section.pages.some((page) => page.total > page.loaded)));
}
