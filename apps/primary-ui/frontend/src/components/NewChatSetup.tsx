import { useState } from "react";
import { AgentContextMenu, type AgentContextTarget } from "./AgentContextMenu";
import { entityOverrideKey } from "../lib/personal-overrides";
import { Star } from "lucide-react";
import { Select, cn } from "@design-system/ui";
import { useChatSelection } from "../lib/app-chat-selection";
import { isLaunchable } from "../lib/app-chat-model";
import { selectedSources, sourceName } from "../lib/app-chat-sources";

export function NewChatSetup() {
  const { agents, newChatAgents, agent, activeSource, sources, newConversationOn, newConversation, agentsLoading, personalAgentOverrides } = useChatSelection();
  const [contextTarget, setContextTarget] = useState<AgentContextTarget | null>(null);
  const personalUnstarred = newChatAgents.filter((item) => personalAgentOverrides[entityOverrideKey("agent", item.name, item.source)]?.starred === false);
  const options = [...agents]
    .sort((a, b) => Number(Boolean(b.starred)) - Number(Boolean(a.starred)) || a.name.localeCompare(b.name));
  return <section data-slot="app-chat-agent" aria-label="Start a chat" className="mx-auto w-full max-w-xl space-y-5 text-left">
    <div><h2 className="text-lg font-semibold">Start a chat</h2><p className="text-muted-foreground mt-1 text-sm">Choose an agent, then write your first message below.</p></div>
    <div className="flex flex-col gap-3 sm:flex-row">
      {selectedSources(sources).length > 1 && <label className="flex-1 space-y-1.5 text-xs font-medium">Machine<Select aria-label="New conversation source" data-slot="app-chat-new-source" value={activeSource ?? ""} onValueChange={(value) => newConversationOn(value || undefined)} options={selectedSources(sources).map((source) => ({ value: source ?? "", label: sourceName(source) }))} /></label>}
      <label className="flex-1 space-y-1.5 text-xs font-medium">Agent<Select data-app-chat-agent="" aria-label="Agent" value={agent} onValueChange={newConversation} placeholder="Choose an agent" disabled={!options.length} options={options.map((item) => ({ value: item.name, label: `${item.starred ? "★ " : ""}${item.displayName ?? item.name}${item.caps?.answerable === false ? " (read only)" : ""}`, disabled: !isLaunchable(item) }))} /></label>
    </div>
    <div className="space-y-2">
      <p className="text-muted-foreground text-xs font-medium">Starred agents</p>
      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        {newChatAgents.filter((item) => item.starred).map((item) => <button key={JSON.stringify([item.source, item.name])} type="button" data-slot="new-chat-starred-agent" data-agent={item.displayName ?? item.name} data-source={item.source ?? ""} aria-pressed={agent === item.name && activeSource === item.source} aria-disabled={item.runnable === false || item.caps?.answerable === false} className={cn("hover:bg-accent focus-visible:ring-ring flex min-w-0 items-start gap-2 rounded-lg border p-3 text-left outline-none focus-visible:ring-2 aria-disabled:cursor-not-allowed aria-disabled:opacity-50", agent === item.name && activeSource === item.source && "border-ring bg-accent")} onContextMenu={(event) => { event.preventDefault(); setContextTarget({ agent: item, x: event.clientX, y: event.clientY, trigger: event.currentTarget }); }} onKeyDown={(event) => {
            if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) { event.preventDefault(); const rect = event.currentTarget.getBoundingClientRect(); setContextTarget({ agent: item, x: rect.left, y: rect.bottom, trigger: event.currentTarget }); }
          }} onClick={() => { if (isLaunchable(item)) newConversationOn(item.source, item.name); }}>
          <Star aria-hidden="true" className="text-muted-foreground mt-0.5 size-3.5 shrink-0 fill-current" />
          <span className="min-w-0"><span className="block break-words text-sm font-medium">{item.displayName ?? item.name}</span><span className="text-muted-foreground block text-xs">{sourceName(item.source)}{item.caps?.answerable === false ? " · Read only" : item.runnable === false ? " · Unavailable" : item.running ? " · Running" : ""}</span></span>
        </button>)}
      </div>
      {agentsLoading && <p role="status" className="text-muted-foreground text-xs">Loading agents…</p>}
      {!agentsLoading && !newChatAgents.some((item) => item.starred) && <p className="text-muted-foreground text-sm">No starred agents in the selected sources. Choose an agent above to start a chat.</p>}
      {personalUnstarred.length > 0 && <details className="text-muted-foreground text-xs"><summary className="cursor-pointer">Unstarred for me ({personalUnstarred.length})</summary><div className="mt-2 space-y-1">{personalUnstarred.map((item) => <div key={entityOverrideKey("agent", item.name, item.source)} className="flex items-center justify-between gap-2"><span>{item.displayName ?? item.name} · {sourceName(item.source)}</span><button type="button" className="hover:text-foreground rounded px-2 py-1 underline" onClick={(event) => { const rect = event.currentTarget.getBoundingClientRect(); setContextTarget({ agent: item, x: rect.left, y: rect.bottom, trigger: event.currentTarget }); }}>Personal preferences</button></div>)}</div></details>}
    </div>
    {contextTarget && <AgentContextMenu target={contextTarget} onClose={() => setContextTarget(null)} />}
  </section>;
}
