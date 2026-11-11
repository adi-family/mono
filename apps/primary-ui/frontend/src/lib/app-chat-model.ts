import type { Agent, AgentPeek, Conversation, ConversationIndex, ConversationRef } from "./app-chat";
import { matchesConversationFilter } from "./app-chat-sidebar";
import {
  conversationKey, hasSource, sameConversation, selectedSources, type ChatFilter, type ChatGroup, type ChatSources, type LastLaunchedAgent,
} from "./app-chat-sources";
import { chatRoute } from "./app-chat-queries";

/** Local edits shown over the server's row until the write and the following read settle. */
export type ChatOverlay = { hidden?: boolean; title?: string; starred?: boolean };
export type ChatOverlays = Record<string, ChatOverlay>;

const fallbackAgentName = "adi-agent";
const previewLength = 120;
const rememberedRowLimit = 1000;

export const isLaunchable = (agent: Agent) => agent.runnable !== false && agent.caps?.answerable !== false;

function byLaunchPreference(a: Agent, b: Agent) {
  return Number(Boolean(b.starred)) - Number(Boolean(a.starred))
    || Number(b.name === fallbackAgentName) - Number(a.name === fallbackAgentName)
    || a.name.localeCompare(b.name);
}

export function preferredAgentName(agents: Agent[]): string | undefined {
  return agents.filter(isLaunchable).sort(byLaunchPreference)[0]?.name;
}

/** The desired agent when it can launch, otherwise the best one available. */
export function chooseAgentName(agents: Agent[], desired?: string): string {
  return agents.find((agent) => agent.name === desired && isLaunchable(agent))?.name ?? preferredAgentName(agents) ?? "";
}

/** An explicit route wins, then the last launch, then the first selected source. */
export function resolveActiveSource(params: URLSearchParams, sources: ChatSources, lastLaunched?: LastLaunchedAgent): string | undefined {
  if (params.has("node")) return params.get("node") || undefined;
  if (params.has("agent") || params.has("run")) return undefined;
  if (lastLaunched && hasSource(sources, lastLaunched.source)) return lastLaunched.source;
  return selectedSources(sources)[0];
}

export function newChatHref(agent: string, source?: string): string {
  return `${chatRoute}?${new URLSearchParams({ agent, ...(source ? { node: source } : {}) })}`;
}

/** One line of text, short enough to stand in for a chat title. */
export function previewText(text: string | undefined): string | undefined {
  return text?.trim().replace(/\s+/g, " ").slice(0, previewLength);
}

/** Identifies a conversation, or the new chat being composed for an agent on a machine. */
export function selectionKeyOf(active: ConversationRef | null, agent: string, source?: string): string {
  return conversationKey(active ?? { agent, source, runId: "new" });
}

export function latestTurnSeq(snapshot: AgentPeek | undefined): number | null {
  return snapshot ? Math.max(-1, ...snapshot.turns.map((turn) => turn.seq)) : null;
}

/** Whether a snapshot shows the reply as stored, after the send itself reported failure. */
export function confirmsReply(snapshot: AgentPeek, afterSeq: number, message: string, attachments: readonly string[]): boolean {
  return snapshot.turns.some((turn) => turn.role === "user"
    && turn.seq > afterSeq
    && turn.text.trim() === message.trim()
    && turn.images.length === attachments.length
    && attachments.every((id) => turn.images.some((image) => image.id === id)));
}

/** Adds rows that are new or changed, evicting the oldest. Returns `current` when nothing changed. */
export function rememberRows(
  current: ReadonlyMap<string, Conversation>, rows: readonly Conversation[], limit = rememberedRowLimit,
): ReadonlyMap<string, Conversation> {
  const changed = rows.filter((row) => current.get(conversationKey(row)) !== row);
  if (!changed.length) return current;
  const next = new Map(current);
  for (const row of changed) {
    const key = conversationKey(row);
    next.delete(key);
    next.set(key, row);
  }
  for (const key of next.keys()) {
    if (next.size <= limit) break;
    next.delete(key);
  }
  return next;
}

/** Every row an index carries, whether listed flat or inside a project group. */
export function indexRows(index: ConversationIndex | undefined): Conversation[] {
  return [...(index?.conversations ?? []), ...(index?.groups ?? []).flatMap((group) => group.conversations)];
}

function activeRowFrom(active: ConversationRef, prior: Conversation | undefined, snapshot: AgentPeek | undefined, overlay: ChatOverlay | undefined): Conversation {
  return {
    ...prior,
    ...active,
    title: prior?.title ?? previewText(snapshot?.turns.find((turn) => turn.role === "user")?.text) ?? "Conversation",
    message: prior?.message ?? "",
    lastActivity: prior?.lastActivity ?? 0,
    running: snapshot?.running ?? prior?.running ?? false,
    answerable: snapshot?.answerable ?? prior?.answerable ?? false,
    interactive: snapshot?.interactive ?? prior?.interactive ?? false,
    pendingQuestion: snapshot ? Boolean(snapshot.pending_question) : prior?.pendingQuestion,
    awaiting: snapshot ? snapshot.awaits.length > 0 : prior?.awaiting,
    ...overlay,
  };
}

export interface SourceRowsInput {
  source?: string;
  index?: ConversationIndex;
  agents: Agent[];
  overlays: ChatOverlays;
  filter: ChatFilter;
  group: ChatGroup;
  active: ConversationRef | null;
  snapshot?: AgentPeek;
  remembered: ReadonlyMap<string, Conversation>;
}

/** Turns one source's index into the rows the sidebar shows. */
export function buildSourceRows({ source, index, agents, overlays, filter, group, active, snapshot, remembered }: SourceRowsInput) {
  const enrich = (row: Conversation): Conversation => {
    const owner = agents.find((agent) => agent.name === row.agent);
    return {
      ...row,
      ...overlays[conversationKey(row)],
      ...(owner ? { agentStarred: owner.starred, agentDisplayName: owner.displayName, project: row.project ?? owner.project } : {}),
    };
  };
  const isShown = (row: Conversation) => !overlays[conversationKey(row)]?.hidden && matchesConversationFilter(row, filter, active);
  const visibleRows = (rows: Conversation[]) => rows.map(enrich).filter(isShown);

  const conversations = visibleRows(index?.conversations ?? []);
  // A narrowing never makes the conversation being read disappear from its own rail.
  if (active && active.source === source && !overlays[conversationKey(active)]?.hidden && !conversations.some((row) => sameConversation(row, active))) {
    const prior = remembered.get(conversationKey(active));
    if (prior || snapshot) conversations.push(activeRowFrom(active, prior, snapshot, overlays[conversationKey(active)]));
  }

  const groups = index?.groups?.map((section) => ({
    ...section, loaded: section.conversations.length, conversations: visibleRows(section.conversations),
  })) ?? [];
  const activeRow = active ? conversations.find((row) => sameConversation(row, active)) : undefined;
  if (group === "project" && activeRow && !groups.some((section) => section.conversations.some((row) => sameConversation(row, activeRow)))) {
    const section = groups.find((candidate) => candidate.project === activeRow.project);
    if (section) section.conversations.push(activeRow);
    else groups.push({ project: activeRow.project, label: activeRow.projectName || "No project", conversations: [activeRow], total: 0, loaded: 0 });
  }

  return { source, conversations, groups, total: index?.total ?? 0, loaded: index?.conversations.length ?? 0 };
}

/** Applies an accepted edit to a cached index, removing the row when the edit hides it. */
export function patchIndex(index: ConversationIndex, key: string, overlay: ChatOverlay): ConversationIndex {
  const patched = (rows: Conversation[]) => rows.flatMap((row) => {
    if (conversationKey(row) !== key) return [row];
    return overlay.hidden ? [] : [{ ...row, ...overlay }];
  });
  return {
    ...index,
    conversations: patched(index.conversations),
    groups: index.groups?.map((section) => ({ ...section, conversations: patched(section.conversations) })),
  };
}

/** Drops one field's overlay, leaving any other edit of that conversation in place. */
export function clearOverlayField(overlays: ChatOverlays, key: string, field: keyof ChatOverlay): ChatOverlays {
  const { [field]: _cleared, ...remaining } = overlays[key] ?? {};
  if (Object.keys(remaining).length) return { ...overlays, [key]: remaining };
  const { [key]: _emptied, ...others } = overlays;
  return others;
}
