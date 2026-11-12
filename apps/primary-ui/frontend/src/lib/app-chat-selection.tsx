import { createContext, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { useLocation, useNavigate, useSearchParams } from "react-router-dom";

import { getAgents, getFleetNodes, type Agent, type ConversationRef } from "./app-chat";
import { errorText } from "./app-chat-errors";
import { chooseAgentName, newChatHref, resolveActiveSource, selectionKeyOf } from "./app-chat-model";
import { agentPollMs, chatQueryKeys, chatRoute, fleetPollMs, queryOutcomes } from "./app-chat-queries";
import {
  groupPreferenceKey, hasSource, lastLaunchedAgentKey, readGroup, readLastLaunchedAgent, readSources, routeNode, savePreference,
  selectedSources, sourcesPreferenceKey, toggledSource, withSource, type ChatFilter, type ChatGroup, type LastLaunchedAgent,
} from "./app-chat-sources";
import { conversationHref } from "./app-chat-transcript";
import { reportEvent } from "./event-log";
import { applyOverrides, entityOverrideKey, usePersonalOverrides } from "./personal-overrides";

const noAgents: Agent[] = [];

function useSelectionState() {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const enabled = pathname === chatRoute;
  const routeSource = routeNode(params.get("node"));
  const invalidSource = params.has("node") && !routeSource;

  const [lastLaunchedAgent, setLastLaunchedAgent] = useState(readLastLaunchedAgent);
  const [sources, setSources] = useState(() => {
    const saved = readSources();
    return routeSource ? withSource(saved, routeSource) : saved;
  });
  const [filter, setFilter] = useState<ChatFilter>("me");
  const [group, setGroupState] = useState<ChatGroup>(readGroup);
  const sourceList = useMemo(() => selectedSources(sources), [sources]);
  const activeSource = resolveActiveSource(params, sources, lastLaunchedAgent);

  // The machine selector sits in every page's topbar, so the fleet is read everywhere; the
  // session reads stay confined to the chat page.
  const fleetQuery = useQuery({ queryKey: chatQueryKeys.fleet, queryFn: ({ signal }) => getFleetNodes(signal), retry: false, refetchInterval: fleetPollMs });
  const fleetError = fleetQuery.error ? errorText(fleetQuery.error) : null;
  // The fleet is polled on every page; a failure goes to the event log once per outage.
  useEffect(() => {
    if (fleetError) reportEvent({ level: "error", area: "Fleet", message: fleetError });
  }, [fleetError]);
  const agentResults = useQueries({
    queries: sourceList.map((source) => ({
      queryKey: chatQueryKeys.agents(source),
      queryFn: ({ signal }: { signal: AbortSignal }) => getAgents(signal, source),
      enabled, refetchInterval: agentPollMs, retry: false,
    })),
    combine: queryOutcomes<Agent[]>,
  });
  const { overrides, setOverride } = usePersonalOverrides();
  const agentsBySource = agentResults.map((result) => (result.data ?? noAgents).map(
    (item) => applyOverrides(item, overrides, entityOverrideKey("agent", item.name, item.source)),
  ));
  const activeSourceIndex = sourceList.indexOf(activeSource);
  const agents = agentsBySource[activeSourceIndex] ?? noAgents;
  const rememberedAgent = lastLaunchedAgent?.source === activeSource ? lastLaunchedAgent?.agent : undefined;
  const agent = params.get("agent") || chooseAgentName(agents, rememberedAgent);
  const runId = params.get("run") ?? "";
  const active = useMemo(
    () => agent && runId ? { agent, runId, ...(activeSource ? { source: activeSource } : {}) } : null,
    [agent, runId, activeSource],
  );
  const selectionKey = selectionKeyOf(active, agent, activeSource);

  // Async work checks this when it settles, because the user may have moved on meanwhile.
  const latestSelection = useRef({ key: selectionKey, pathname });
  useLayoutEffect(() => { latestSelection.current = { key: selectionKey, pathname }; }, [selectionKey, pathname]);

  useEffect(() => { savePreference(sourcesPreferenceKey, sources); }, [sources]);
  // Deep links and browser history select their owning machine as well as the conversation.
  useEffect(() => {
    if (enabled && !invalidSource) setSources((current) => hasSource(current, activeSource) ? current : withSource(current, activeSource));
  }, [enabled, activeSource, invalidSource]);

  function openConversation(ref: ConversationRef) { navigate(conversationHref(ref)); }

  function savedLaunch() { return readLastLaunchedAgent() ?? lastLaunchedAgent; }

  function navigateToNewChat(source: string | undefined, name: string | undefined, saved: LastLaunchedAgent | undefined) {
    const available = agentResults[sourceList.indexOf(source)]?.data;
    const desired = name ?? (saved?.source === source ? saved?.agent : undefined);
    // An empty explicit agent keeps this a new chat while the destination's agents load.
    navigate(newChatHref(available ? chooseAgentName(available, desired) : desired ?? "", source));
  }

  /** Starts a new chat, inferring the machine from the agent choice and the last launch. */
  function newConversation(name?: string) {
    const saved = savedLaunch();
    navigateToNewChat(!name && saved && hasSource(sources, saved.source) ? saved.source : activeSource, name, saved);
  }

  /** Starts a new chat on one specific machine; `undefined` means this machine. */
  function newConversationOn(source: string | undefined, name?: string) {
    navigateToNewChat(source, name, savedLaunch());
  }

  function toggleSource(source?: string) {
    const next = toggledSource(sources, source);
    setSources(next);
    // Only the chat page has an open conversation to move off a deselected machine; on Home the
    // same control just changes whose apps are listed, and must not navigate away.
    if (enabled && !hasSource(next, activeSource)) newConversationOn(selectedSources(next)[0]);
  }

  function setGroup(value: ChatGroup) {
    setGroupState(value);
    savePreference(groupPreferenceKey, value);
  }

  function recordLaunch(launched: LastLaunchedAgent) {
    setLastLaunchedAgent(launched);
    savePreference(lastLaunchedAgentKey, launched);
  }

  return {
    enabled, invalidSource, sources, sourceList, activeSource, toggleSource,
    filter, setFilter, group, setGroup,
    fleetNodes: fleetQuery.data ?? [], fleetLoading: fleetQuery.isPending, fleetError,
    agents, agentsBySource, agentResults, newChatAgents: agentsBySource.flat(),
    agentsLoading: agentResults.some((result) => result.isPending),
    activeAgentsPending: agentResults[activeSourceIndex]?.isPending === true,
    personalAgentOverrides: overrides,
    setAgentOverride: <K extends keyof Agent>(item: Agent, field: K, value: Agent[K] | undefined) => setOverride(entityOverrideKey("agent", item.name, item.source), field, value),
    agent, runId, active, selectionKey,
    routeNamesConversation: params.has("agent") || params.has("run"),
    isCurrentSelection: (key: string) => latestSelection.current.key === key && latestSelection.current.pathname === chatRoute,
    openConversation, newConversation, newConversationOn, recordLaunch,
  };
}

const ChatSelectionContext = createContext<ReturnType<typeof useSelectionState> | null>(null);

export function ChatSelectionProvider({ children }: { children: ReactNode }) {
  return <ChatSelectionContext.Provider value={useSelectionState()}>{children}</ChatSelectionContext.Provider>;
}

/** Which machines, agent and conversation are selected. Stable while a transcript polls or a draft changes. */
export function useChatSelection() {
  const value = useContext(ChatSelectionContext);
  if (!value) throw new Error("useChatSelection must be inside AppChatProvider.");
  return value;
}
