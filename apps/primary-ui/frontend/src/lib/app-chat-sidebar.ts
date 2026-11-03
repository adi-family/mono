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

export interface SidebarSourceState {
  source?: string;
  conversations: Conversation[];
  total: number;
  held?: number;
  groups?: { project?: string; label: string; conversations: Conversation[]; total: number; held?: number }[];
  isLoading: boolean;
  error: string | null;
}

interface SidebarPage {
  source?: string;
  project?: string;
  total: number;
  held: number;
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
  const pages = sources.map((source) => ({ source: source.source, total: source.total, held: source.held ?? source.conversations.length }));

  if (group === "flat") {
    return [{ key: "all", conversations: visible, pages }];
  }
  if (group === "machine") {
    return sources.map((source) => ({
      key: JSON.stringify([source.source ?? ""]),
      label: source.source ?? "This machine",
      conversations: visible.filter((conversation) => conversation.source === source.source),
      pages: [{ source: source.source, total: source.total, held: source.held ?? source.conversations.length }],
    })).filter((section) => section.conversations.length > 0 || (!search && section.pages.some((page) => page.total > page.held)));
  }

  const sections = new Map<string, ConversationSection>();
  for (const source of sources) {
    for (const project of source.groups ?? []) {
      const section = sections.get(project.label) ?? { key: `project:${project.label}`, label: project.label, conversations: [], pages: [] };
      section.pages.push({ source: source.source, project: project.project ?? "~", total: project.total, held: project.held ?? project.conversations.length });
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
  return [...sections.values()].filter((section) => section.conversations.length > 0 || (!search && section.pages.some((page) => page.total > page.held)));
}
