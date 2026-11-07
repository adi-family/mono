import { createContext, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useLocation, useNavigate, useSearchParams } from "react-router-dom";

import {
  answerQuestion, getAgents, getConversation, getConversationIndex, getFleetNodes, replyToConversation,
  startConversation, stopConversation, hideConversation, renameConversation, starConversation, uploadImage, attachmentUrl,
  type Agent, type Conversation, type ConversationIndex, type ConversationRef,
} from "./app-chat";
import { conversationHref } from "./app-chat-transcript";
import { matchesConversationFilter } from "./app-chat-sidebar";
import {
  conversationKey, hasSource, normalizeSources, readGroup, readSources, routeNode,
  readLastLaunchedAgent, lastLaunchedAgentKey, savePreference, selectedSources, sourceName, type ChatFilter, type ChatGroup,
} from "./app-chat-sources";

import { applyOverrides, entityOverrideKey, usePersonalOverrides } from "./personal-overrides";

type DraftImage = { id: string; name: string; mediaType?: string; url?: string; attachmentId?: string; error?: string };
const emptyAgents: Agent[] = [];
const identity = (ref: ConversationRef | null, agent: string, source?: string) => conversationKey(ref ?? { agent, source, runId: "new" });
const errorText = (error: unknown) => error instanceof Error ? error.message : "The app could not complete this request.";

function useAppChatState() {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const client = useQueryClient();
  const enabled = pathname === "/interface";
  const routeSource = routeNode(params.get("node"));
  const invalidSource = params.has("node") && !routeSource;
  const [lastLaunchedAgent, setLastLaunchedAgent] = useState(readLastLaunchedAgent);
  const [sources, setSources] = useState(() => {
    const saved = readSources();
    return routeSource ? normalizeSources({ ...saved, nodes: [...saved.nodes, routeSource] }) : saved;
  });
  const activeSource = params.has("node") ? params.get("node") || undefined : params.has("agent") || params.has("run") ? undefined : lastLaunchedAgent && hasSource(sources, lastLaunchedAgent.source) ? lastLaunchedAgent.source : selectedSources(sources)[0];
  const [filter, setFilter] = useState<ChatFilter>("me");
  const [group, updateGroup] = useState<ChatGroup>(readGroup);
  const sourceList = useMemo(() => selectedSources(sources), [sources]);
  const [indexLimits, setIndexLimits] = useState<Record<string, number>>({});
  const [projectMore, setProjectMore] = useState<Record<string, Record<string, number>>>({});
  const fleetQuery = useQuery({ queryKey: ["app-chat", "fleet"], queryFn: ({ signal }) => getFleetNodes(signal), enabled, retry: false, refetchInterval: 15000 });
  const agentQueries = useQueries({ queries: sourceList.map((source) => ({
    queryKey: ["app-chat", "agents", source ?? ""], queryFn: ({ signal }: { signal: AbortSignal }) => getAgents(signal, source),
    enabled, refetchInterval: 5000, retry: false,
  })) });
  const indexQueries = useQueries({ queries: sourceList.map((source) => ({
    queryKey: ["app-chat", "conversations", source ?? "", filter, group, indexLimits[source ?? ""] ?? 100, projectMore[source ?? ""] ?? {}],
    queryFn: ({ signal }: { signal: AbortSignal }) => getConversationIndex(signal, source, {
      filter, limit: indexLimits[source ?? ""] ?? 100,
      ...(group === "project" ? { group: "project" as const, groupLimit: 3, groupMore: projectMore[source ?? ""] } : {}),
    }),
    enabled, refetchInterval: 5000, retry: false,
  })) });
  const { overrides, setOverride } = usePersonalOverrides();
  const effectiveAgents = agentQueries.map((query) => (query.data ?? emptyAgents).map((item) => applyOverrides(item, overrides, entityOverrideKey("agent", item.name, item.source))));
  const currentSourceIndex = sourceList.indexOf(activeSource);
  const agents = effectiveAgents[currentSourceIndex] ?? emptyAgents;
  const preferred = (items: Agent[]) => [...items].filter((item) => item.runnable !== false && item.caps?.answerable !== false).sort((a, b) => Number(Boolean(b.starred)) - Number(Boolean(a.starred)) || Number(b.name === "adi-agent") - Number(a.name === "adi-agent") || a.name.localeCompare(b.name))[0]?.name;
  const defaultAgent = agents.find((item) => lastLaunchedAgent?.source === activeSource && item.name === lastLaunchedAgent?.agent && item.runnable !== false && item.caps?.answerable !== false)?.name ?? preferred(agents) ?? "";
  const agent = params.get("agent") || defaultAgent;
  const runId = params.get("run") ?? "";
  const active = useMemo(() => agent && runId ? { agent, runId, ...(activeSource ? { source: activeSource } : {}) } : null, [agent, runId, activeSource]);
  const draftKey = identity(active, agent, activeSource);
  const selection = useRef({ key: draftKey, pathname });
  selection.current = { key: draftKey, pathname };
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [imageDrafts, setImageDrafts] = useState<Record<string, DraftImage[]>>({});
  const imageDraftsRef = useRef(imageDrafts);
  const nextImageId = useRef(0);
  function updateImages(key: string, update: (images: DraftImage[]) => DraftImage[]) {
    const next = { ...imageDraftsRef.current, [key]: update(imageDraftsRef.current[key] ?? []) };
    imageDraftsRef.current = next;
    setImageDrafts(next);
  }
  const [imageErrors, setImageErrors] = useState<Record<string, string>>({});
  function addImages(files: File[]) {
    const key = draftKey;
    const source = activeSource;
    const room = Math.max(0, 6 - (imageDraftsRef.current[key]?.length ?? 0));
    setImageErrors((current) => ({ ...current, [key]: files.length > room ? "Attach up to six files per message." : "" }));
    for (const file of files.slice(0, room)) {
      const id = `draft-image-${++nextImageId.current}`;
      updateImages(key, (images) => [...images, { id, name: file.name }]);
      void uploadImage(file, source).then((image) => {
        updateImages(key, (images) => images.map((item) => item.id === id ? { ...item, attachmentId: image.id, mediaType: image.media_type, url: attachmentUrl(image.id, source) } : item));
      }).catch((error) => {
        updateImages(key, (images) => images.map((item) => item.id === id ? { ...item, error: errorText(error) } : item));
      });
    }
  }
  const [limits, setLimits] = useState<Record<string, number>>({});
  const limit = limits[draftKey] ?? 20;
  const snapshotKey = ["app-chat", "conversation", activeSource ?? "", agent, runId, limit] as const;
  const snapshotQuery = useQuery({
    queryKey: snapshotKey,
    queryFn: ({ signal }) => getConversation({ agent, runId, source: activeSource }, signal, { limit }),
    enabled: enabled && !invalidSource && active !== null && hasSource(sources, activeSource), retry: false,
    placeholderData: (previous, previousQuery) => previousQuery?.queryKey[2] === (activeSource ?? "") && previousQuery.queryKey[3] === agent && previousQuery.queryKey[4] === runId ? previous : undefined,
    refetchInterval: (query) => query.state.data?.running || query.state.data?.turns.some((turn) => turn.pending || turn.queued) ? 1000 : 5000,
  });
  const snapshot = active && !invalidSource ? snapshotQuery.data : undefined;
  type ChatPatch = { hidden?: boolean; title?: string; starred?: boolean };
  const [chatPatches, setChatPatches] = useState<Record<string, ChatPatch>>({});
  const pendingEdits = useRef(new Set<string>());
  const [mutationError, setMutationError] = useState<string | null>(null);
  const known = useRef(new Map<string, Conversation>());
  const sourceStates = sourceList.map((source, index) => {
    const query = indexQueries[index];
    const data = query.data;
    const enrich = (conversation: Conversation) => {
      const owner = effectiveAgents[index]?.find((item) => item.name === conversation.agent);
      const result = { ...conversation, ...chatPatches[conversationKey(conversation)], ...(owner ? { agentStarred: owner.starred, agentDisplayName: owner.displayName, project: conversation.project ?? owner.project } : {}) };
      known.current.set(conversationKey(result), conversation);
      return result;
    };
    const conversations = (data?.conversations ?? []).map(enrich).filter((item) => !chatPatches[conversationKey(item)]?.hidden && matchesConversationFilter(item, filter, active));
    // A narrowing never makes the conversation being read disappear from its own rail.
    if (active && !chatPatches[conversationKey(active)]?.hidden && active.source === source && !conversations.some((item) => conversationKey(item) === conversationKey(active))) {
      const prior = known.current.get(conversationKey(active));
      if (prior || snapshot) conversations.push({
        ...prior,
        ...active, title: prior?.title ?? snapshot?.turns.find((turn) => turn.role === "user")?.text.trim().replace(/\s+/g, " ").slice(0, 120) ?? "Conversation",
        message: prior?.message ?? "", lastActivity: prior?.lastActivity ?? 0,
        running: snapshot?.running ?? prior?.running ?? false,
        answerable: snapshot?.answerable ?? prior?.answerable ?? false,
        interactive: snapshot?.interactive ?? prior?.interactive ?? false,
        pendingQuestion: snapshot ? Boolean(snapshot.pending_question) : prior?.pendingQuestion,
        awaiting: snapshot ? snapshot.awaits.length > 0 : prior?.awaiting,
        ...chatPatches[conversationKey(active)],
      });
    }
    const groups = data?.groups?.map((item) => ({ ...item, held: item.conversations.length, conversations: item.conversations.map(enrich).filter((row) => !chatPatches[conversationKey(row)]?.hidden && matchesConversationFilter(row, filter, active)) })) ?? [];
    const exempt = active && conversations.find((item) => conversationKey(item) === conversationKey(active));
    if (group === "project" && exempt && !groups.some((item) => item.conversations.some((row) => conversationKey(row) === conversationKey(exempt)))) {
      const section = groups.find((item) => item.project === exempt.project);
      if (section) section.conversations.push(exempt);
      else groups.push({ project: exempt.project, label: exempt.projectName || "No project", conversations: [exempt], total: 0, held: 0 });
    }
    const error = query.error ?? agentQueries[index]?.error;
    return { source, conversations, groups, total: data?.total ?? 0, held: data?.conversations.length ?? 0, isLoading: query.isPending, isFetching: query.isFetching, error: error ? errorText(error) : null };
  });
  const conversations = sourceStates.flatMap((item) => item.conversations).sort((a, b) => b.lastActivity - a.lastActivity);
  const sending = useRef(new Set<string>());
  const [stopping, setStopping] = useState<string | null>(null);
  const [actionError, setActionError] = useState<{ key: string; message: string } | null>(null);

  useEffect(() => { savePreference("adi-session-sources", sources); }, [sources]);
  // Deep links and browser history select their owning machine as well as the conversation.
  useEffect(() => {
    if (enabled && !invalidSource) setSources((current) => hasSource(current, activeSource) ? current : normalizeSources(activeSource ? { ...current, nodes: [...current.nodes, activeSource] } : { ...current, local: true }));
  }, [enabled, activeSource, invalidSource]);

  function openConversation(ref: ConversationRef) { navigate(conversationHref(ref)); }
  function newConversation(name?: string, ...sourceArgument: [source?: string]) {
    const saved = readLastLaunchedAgent() ?? lastLaunchedAgent;
    const source = sourceArgument.length ? sourceArgument[0] : name ? activeSource : saved && hasSource(sources, saved.source) ? saved.source : activeSource;
    const available = agentQueries[sourceList.indexOf(source)]?.data;
    const desired = name ?? (saved?.source === source ? saved?.agent : undefined);
    const chosen = available ? available.find((item) => item.name === desired && item.runnable !== false && item.caps?.answerable !== false)?.name ?? preferred(available) ?? "" : desired ?? "";
    // An empty explicit agent keeps this a new chat while the destination's agents load.
    navigate(`/interface?${new URLSearchParams({ agent: chosen, ...(source ? { node: source } : {}) })}`);
  }
  function toggleSource(source?: string) {
    const next = normalizeSources(source ? { ...sources, nodes: sources.nodes.includes(source) ? sources.nodes.filter((node) => node !== source) : [...sources.nodes, source] } : { ...sources, local: !sources.local });
    setSources(next);
    if (!hasSource(next, activeSource)) newConversation(undefined, selectedSources(next)[0]);
  }
  function setGroup(value: ChatGroup) { updateGroup(value); savePreference("adi-session-group", value); }
  function loadMore(source?: string, project?: string) {
    if (project !== undefined) setProjectMore((current) => ({ ...current, [source ?? ""]: { ...current[source ?? ""], [project]: (current[source ?? ""]?.[project] ?? 0) + 3 } }));
    else setIndexLimits((current) => ({ ...current, [source ?? ""]: (current[source ?? ""] ?? 100) + 100 }));
  }

  useEffect(() => {
    if (!window.matchMedia("(max-width: 767px)").matches && enabled && !params.has("agent") && !params.has("run") && conversations.length && !drafts[draftKey] && !imageDrafts[draftKey]?.length && !sending.current.has(draftKey)) {
      const recent = conversations.find((item) => item.source === activeSource);
      if (recent) navigate(conversationHref(recent), { replace: true });
    }
  }, [enabled, params, conversations, navigate, drafts, imageDrafts, draftKey, activeSource]);

  async function refresh() {
    setActionError(null);
    await Promise.all([client.invalidateQueries({ queryKey: ["app-chat", "voice"] }), client.invalidateQueries({ queryKey: ["app-chat", "agents"] }), client.invalidateQueries({ queryKey: ["app-chat", "conversations"] }), client.invalidateQueries({ queryKey: ["app-chat", "conversation"] }), client.invalidateQueries({ queryKey: ["app-chat", "fleet"] })]);
  }
  async function sendMessage(message: string, mode: "regular" | "asap" = "regular") {
    if (invalidSource) throw new Error("This chat source is invalid. Choose a source from the sidebar.");
    if (!agent) throw new Error("Choose an agent before sending a message.");
    if (sending.current.has(draftKey)) throw new Error("A message is already being sent to this conversation.");
    const images = imageDraftsRef.current[draftKey] ?? [];
    if (images.some((image) => !image.attachmentId)) throw new Error("Wait for files to upload, or remove failed attachments.");
    if (!message.trim() && !images.length) throw new Error("Write a message or attach a file.");
    const attachments = images.map((image) => image.attachmentId!);
    const target = active;
    const submittedDraft = drafts[draftKey] ?? "";
    const key = draftKey;
    const baselineSeq = snapshot ? Math.max(-1, ...snapshot.turns.map((turn) => turn.seq)) : null;
    sending.current.add(key);
    try {
      if (target) {
        let result;
        try { result = await replyToConversation(target, message, undefined, attachments, mode); }
        catch (sendError) {
          // The write can succeed before snapshot generation or delivery fails.
          // Reconcile with a read; never repeat the mutation automatically.
          const confirmed = baselineSeq === null ? undefined : await getConversation(target).catch(() => undefined);
          const delivered = confirmed?.turns.some((turn) => turn.role === "user" && turn.seq > baselineSeq! && turn.text.trim() === message.trim()
            && turn.images.length === attachments.length && attachments.every((id) => turn.images.some((image) => image.id === id)));
          if (!delivered) throw sendError;
          result = confirmed!;
        }
        if (limit <= 20) client.setQueryData(snapshotKey, result);
        setDrafts((current) => current[key] === submittedDraft ? { ...current, [key]: "" } : current);
      } else {
        const created = await startConversation(agent, message, undefined, activeSource, attachments);
        const launched = { agent: created.agent, ...(created.source ? { source: created.source } : {}) };
        setLastLaunchedAgent(launched);
        savePreference(lastLaunchedAgentKey, launched);
        const createdKey = identity(created, created.agent, created.source);
        setDrafts((current) => ({ ...current, [key]: "", [createdKey]: current[key] === submittedDraft ? "" : current[key] ?? "" }));
        if (selection.current.key === key && selection.current.pathname === "/interface") openConversation(created);
      }
      updateImages(key, (current) => current.filter((image) => !images.some((submitted) => submitted.id === image.id)));
      void refresh();
    } catch (error) {
      setDrafts((current) => current[key] ? current : { ...current, [key]: submittedDraft });
      throw error;
    } finally { sending.current.delete(key); }
  }
  async function stopResponse() {
    if (!active || stopping === draftKey) return;
    const key = draftKey;
    setStopping(key); setActionError(null);
    try { await stopConversation(active); await refresh(); }
    catch (error) { setActionError({ key, message: errorText(error) }); }
    finally { setStopping((current) => current === key ? null : current); }
  }
  async function submitAnswer(ask: string, replies: string[]) {
    if (!active) throw new Error("Open a conversation first.");
    const result = await answerQuestion(active, ask, replies);
    if (limit <= 20) client.setQueryData(snapshotKey, result);
    void refresh();
  }
  // Keep each field overlaid through the write and subsequent reads. Polling cannot
  // flash stale values, and rolling back one field preserves other pending edits.
  async function editChat(ref: ConversationRef, patch: ChatPatch, write: () => Promise<void>, failure: string) {
    const key = conversationKey(ref);
    const field = Object.keys(patch)[0] as keyof ChatPatch;
    const editKey = `${key}:${field}`;
    if (pendingEdits.current.has(editKey)) return;
    pendingEdits.current.add(editKey);
    setMutationError(null);
    setChatPatches((current) => ({ ...current, [key]: { ...current[key], ...patch } }));
    try {
      await write();
      if (patch.hidden && selection.current.key === key && selection.current.pathname === "/interface") newConversation(ref.agent, ref.source);
      // Update inactive filter/group caches too. A failed follow-up read must not
      // roll back a write the server has already accepted.
      client.setQueriesData<ConversationIndex>({ queryKey: ["app-chat", "conversations"] }, (data) => {
        if (!data) return data;
        const update = (rows: Conversation[]) => rows.flatMap((row) => conversationKey(row) !== key ? [row] : patch.hidden ? [] : [{ ...row, ...patch }]);
        return { ...data, conversations: update(data.conversations), groups: data.groups?.map((section) => ({ ...section, conversations: update(section.conversations) })) };
      });
      await client.invalidateQueries({ queryKey: ["app-chat", "conversations"] });
    } catch (error) {
      setMutationError(`${failure} ${errorText(error)}`);
    } finally {
      setChatPatches((current) => {
        const remaining = { ...current[key] };
        delete remaining[field];
        const next = { ...current, [key]: remaining };
        if (!Object.keys(remaining).length) delete next[key];
        return next;
      });
      pendingEdits.current.delete(editKey);
    }
  }
  return {
    newChatAgents: effectiveAgents.flat(),
    personalAgentOverrides: overrides,
    setAgentOverride: <K extends keyof Agent>(item: Agent, field: K, value: Agent[K] | undefined) => setOverride(entityOverrideKey("agent", item.name, item.source), field, value),
    agentsLoading: agentQueries.some((query) => query.isPending),
    agents, conversations, sourceStates, sources, activeSource, filter, setFilter, group, setGroup, toggleSource, loadMore,
    fleetNodes: fleetQuery.data ?? [], fleetLoading: fleetQuery.isPending, fleetError: fleetQuery.error ? errorText(fleetQuery.error) : null,
    setNewSource: (source?: string) => newConversation(undefined, source),
    active, agent, draftKey, snapshot, draft: drafts[draftKey] ?? "",
    images: imageDrafts[draftKey] ?? [], imageError: imageErrors[draftKey] || null, addImages,
    removeImage: (id: string) => updateImages(draftKey, (images) => images.filter((image) => image.id !== id)),
    setDraft: (value: string) => setDrafts((current) => ({ ...current, [draftKey]: value })),
    isLoading: agentQueries[currentSourceIndex]?.isPending === true || indexQueries.some((query) => query.isPending),
    isLoadingConversation: active !== null && !invalidSource && snapshotQuery.isPending,
    indexError: sourceStates.filter((item) => item.error).map((item) => `${sourceName(item.source)}: ${item.error}`).join(" ") || null,
    conversationError: invalidSource ? "This chat source is invalid. Choose a source from the sidebar." : active && snapshotQuery.error ? errorText(snapshotQuery.error) : null,
    actionError: actionError?.key === draftKey ? actionError.message : null, isStopping: stopping === draftKey,
    mutationError, dismissMutationError: () => setMutationError(null),
    hideChat: (ref: ConversationRef) => editChat(ref, { hidden: true }, () => hideConversation(ref), "Could not hide this chat."),
    starChat: (ref: ConversationRef, starred: boolean) => editChat(ref, { starred }, () => starConversation(ref, starred), "Could not update the chat star."),
    renameChat: (ref: ConversationRef, title: string) => {
      const prior = known.current.get(conversationKey(ref));
      const defaultTitle = prior?.message.trim().replace(/\s+/g, " ").slice(0, 120) || "Untitled chat";
      return editChat(ref, { title: title.trim() || defaultTitle }, () => renameConversation(ref, title), "Could not rename this chat.");
    },
    appendDraft: (text: string) => setDrafts((current) => ({ ...current, [draftKey]: [current[draftKey]?.trimEnd(), text.trim()].filter(Boolean).join(" ") })),
    openConversation, newConversation, refresh, sendMessage, stopResponse, submitAnswer,
    loadEarlier: () => setLimits((current) => ({ ...current, [draftKey]: limit + 20 })),
    isLoadingEarlier: snapshotQuery.isFetching && snapshotQuery.isPlaceholderData,
  };
}
const AppChatContext = createContext<ReturnType<typeof useAppChatState> | null>(null);
export function AppChatProvider({ children }: { children: ReactNode }) {
  const state = useAppChatState();
  return <AppChatContext.Provider value={state}>{children}{state.mutationError && <div role="alert" data-slot="chat-mutation-error" className="bg-popover text-popover-foreground border-border fixed right-4 bottom-4 z-50 flex max-w-sm items-start gap-3 rounded-md border p-3 text-sm shadow-md">
    <p>{state.mutationError}</p><button type="button" aria-label="Dismiss chat error" className="shrink-0 underline" onClick={state.dismissMutationError}>Dismiss</button>
  </div>}</AppChatContext.Provider>;
}
export function useAppChat() {
  const value = useContext(AppChatContext);
  if (!value) throw new Error("The app chat must be inside AppChatProvider.");
  return value;
}
