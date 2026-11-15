import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { ArrowLeft, LoaderCircle, Play, Plus, RotateCw, Search, Trash2, X, Zap } from "lucide-react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";

import { Badge, Button, Card, SettingsSection, Switch, cn } from "@design-system/ui";

import { sourceName } from "../../lib/app-chat-sources";
import { useSettingsSource } from "../../lib/settings-source";
import {
  enabledBody, filterTriggers, formatAge, loadTriggers, newTriggerHref, runtimeOrDefault, settingsRequest, triggerHref, triggerLogKey,
  triggerStatus, triggersKey, webhookPath,
  type SaveTrigger, type Trigger, type TriggerLog, type TriggerRunResult, type TriggersState,
} from "../../lib/triggers";
import { ConfirmDialog } from "./ConfirmDialog";
import { TriggerEditor } from "./TriggerEditor";

const description = "Code blocks this machine launches on a webhook call, on a platform event, or keeps running in the background.";
const statusVariant = { running: "success", listening: "success", stopped: "warning", disabled: "muted" } as const;

function StatusBadge({ trigger }: { trigger: Trigger }) {
  const status = triggerStatus(trigger);
  return <Badge data-slot="trigger-status" data-status={status.tone} variant={statusVariant[status.tone]} title={status.detail}>{status.label}</Badge>;
}

function fullDate(unixSeconds: number) {
  return new Date(unixSeconds * 1000).toLocaleString();
}

/**
 * Every write answers with the machine's fresh trigger list, which replaces the cached one, so
 * the page never waits for its next poll to show what a save did. One write at a time.
 */
function useTriggerActions(source?: string) {
  const cache = useQueryClient();
  const key = triggersKey(source);
  const pending = useRef(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // A message about a write that also moves the page — created, deleted — outlives that one move.
  const carried = useRef(false);

  async function run<T>(label: string, path: string, body: unknown, stateOf: (result: T) => TriggersState): Promise<T | null> {
    if (pending.current) return null;
    pending.current = true;
    setBusy(label); setError(null); setNotice(null);
    try {
      const result = await settingsRequest<T>(source, path, body);
      await cache.cancelQueries({ queryKey: key });
      cache.setQueryData(key, stateOf(result));
      return result;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The request failed. Try again.");
      return null;
    } finally {
      pending.current = false;
      setBusy(null);
    }
  }

  return {
    busy, error, notice,
    announce: (message: string, carry = false) => { carried.current = carry; setNotice(message); },
    clear: () => { setError(null); setNotice(null); },
    /** The page moved to another view: a result message belongs to the view it was shown in. */
    moved: () => {
      if (carried.current) { carried.current = false; return; }
      setError(null); setNotice(null);
    },
    save: (body: SaveTrigger) => run<TriggersState>(`save:${body.name}`, "/triggers/save", body, (state) => state),
    remove: (name: string) => run<TriggersState>(`delete:${name}`, "/triggers/delete", { name }, (state) => state),
    fire: (name: string) => run<TriggerRunResult>(`fire:${name}`, "/triggers/fire", { name }, (result) => result.state),
    restart: (name: string) => run<TriggerRunResult>(`restart:${name}`, "/triggers/restart", { name }, (result) => result.state),
  };
}
type TriggerActions = ReturnType<typeof useTriggerActions>;

function LoadProblem({ machine, query }: { machine: string; query: UseQueryResult<TriggersState> }) {
  return (
    <div role="alert" className="rounded-xl border p-6">
      <p>Couldn’t load the triggers on {machine}.</p>
      <p className="text-muted-foreground mt-1 text-sm">{query.error instanceof Error ? query.error.message : "The request failed."}</p>
      <Button variant="outline" className="mt-4" onClick={() => void query.refetch()}>Try again</Button>
    </div>
  );
}

function TriggerRow({ trigger, actions }: { trigger: Trigger; actions: TriggerActions }) {
  const status = triggerStatus(trigger);
  const enabled = trigger.enabled !== false;
  const saving = actions.busy === `save:${trigger.name}`;
  return (
    <li data-slot="trigger-row" data-trigger={trigger.name} className="flex flex-wrap items-center gap-x-4 gap-y-3 p-4">
      <div className="min-w-0 flex-1 basis-72">
        <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1.5">
          <Link to={triggerHref(trigger.name)} className="focus-visible:ring-ring rounded-sm font-mono text-sm font-medium break-all underline-offset-4 outline-none hover:underline focus-visible:ring-2">{trigger.name}</Link>
          <StatusBadge trigger={trigger} />
        </div>
        {trigger.description && <p className="text-muted-foreground mt-1.5 line-clamp-2 text-sm leading-relaxed" title={trigger.description}>{trigger.description}</p>}
        <p className="text-muted-foreground mt-2 flex flex-wrap gap-x-3 gap-y-1 text-xs">
          <span>{trigger.kind} · <span className="font-mono">{runtimeOrDefault(trigger.runtime)}</span></span>
          {trigger.project && <span>Project <span className="text-foreground">{trigger.project}</span></span>}
          {trigger.kind === "webhook" && <code className="font-mono break-all">{webhookPath(trigger.name)}</code>}
          {trigger.kind === "event" && !!trigger.events?.length && <code className="font-mono break-all">{trigger.events.join(", ")}</code>}
          {status.detail && status.tone === "running" && <span className="tabular-nums">{status.detail}</span>}
          {trigger.last_fired_at ? <span title={fullDate(trigger.last_fired_at)}>Last run {formatAge(trigger.last_fired_at)}</span> : <span>Never run</span>}
        </p>
      </div>
      {/* On a phone the actions take their own line: the links under the text, the switch at the edge. */}
      <div className="flex shrink-0 items-center gap-1 max-sm:-ml-2.5 max-sm:w-[calc(100%_+_0.625rem)]">
        <Button asChild variant="ghost" size="sm"><Link to={triggerHref(trigger.name, "log")} aria-label={`Log of ${trigger.name}`}>Log</Link></Button>
        <Button asChild variant="ghost" size="sm"><Link to={triggerHref(trigger.name)} aria-label={`Edit ${trigger.name}`}>Edit</Link></Button>
        <span className="ml-2 flex items-center max-sm:ml-auto">
          {saving && <LoaderCircle className="text-muted-foreground mr-2 size-3.5 motion-safe:animate-spin" aria-hidden="true" />}
          <Switch
            aria-label={`${trigger.name} enabled`}
            title={enabled ? "Enabled — switch off to disable" : "Disabled — switch on to enable"}
            checked={enabled}
            disabled={actions.busy !== null}
            onCheckedChange={(next) => void actions.save(enabledBody(trigger, next))}
          />
        </span>
      </div>
    </li>
  );
}

function TriggerList({ machine, query, actions }: { machine: string; query: UseQueryResult<TriggersState>; actions: TriggerActions }) {
  const [search, setSearch] = useState("");
  const triggers = query.data?.triggers ?? [];
  const shown = filterTriggers(triggers, search);
  const running = triggers.filter((trigger) => trigger.running).length;

  return (
    <SettingsSection
      data-slot="settings-triggers"
      title="Triggers"
      description={description}
      actions={<Button asChild data-slot="trigger-new"><Link to={newTriggerHref}><Plus className="size-4" aria-hidden="true" />New trigger</Link></Button>}
    >
      {actions.notice && <p role="status" data-slot="trigger-notice" className="bg-muted mb-4 rounded-lg border p-3 text-sm">{actions.notice}</p>}
      {actions.error && <p role="alert" className="text-destructive mb-4 rounded-lg border p-3 text-sm">{actions.error}</p>}
      {query.isPending ? (
        <p role="status" className="text-muted-foreground flex items-center gap-2 py-12 text-sm"><LoaderCircle className="size-4 motion-safe:animate-spin" aria-hidden="true" /> Loading triggers…</p>
      ) : query.isError && !query.data ? (
        <LoadProblem machine={machine} query={query} />
      ) : triggers.length === 0 ? (
        <div data-slot="triggers-empty" className="rounded-xl border border-dashed px-6 py-14 text-center">
          <Zap className="text-muted-foreground mx-auto mb-4 size-6" aria-hidden="true" />
          <h4 className="font-medium">No triggers on {machine} yet</h4>
          <p className="text-muted-foreground mx-auto mt-2 max-w-sm text-sm leading-relaxed">Start from a preset — a Telegram bot, a webhook, a job that runs every N seconds — or write your own code block.</p>
          <Button asChild className="mt-5"><Link to={newTriggerHref}><Plus className="size-4" aria-hidden="true" />New trigger</Link></Button>
        </div>
      ) : (
        <>
          <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
            <p role="status" data-slot="triggers-count" className="text-muted-foreground text-xs">
              {search.trim() ? `${shown.length} of ${triggers.length}` : triggers.length} {triggers.length === 1 ? "trigger" : "triggers"}{running ? ` · ${running} running` : ""}
            </p>
            {triggers.length > 5 && (
              <div className="focus-within:ring-ring flex w-full items-center gap-2 rounded-lg border px-3 focus-within:ring-2 sm:w-64">
                <Search className="text-muted-foreground size-4 shrink-0" aria-hidden="true" />
                <input aria-label="Search triggers" placeholder="Search triggers…" value={search} onChange={(event) => setSearch(event.target.value)} className="placeholder:text-muted-foreground h-8 w-full min-w-0 bg-transparent text-sm outline-none" />
                {search && <button type="button" aria-label="Clear search" className="focus-visible:ring-ring rounded-sm outline-none focus-visible:ring-2" onClick={() => setSearch("")}><X className="text-muted-foreground size-4" aria-hidden="true" /></button>}
              </div>
            )}
          </div>
          {query.isError && <p role="alert" className="text-muted-foreground mb-3 text-xs">Couldn’t refresh just now; showing the last list that loaded.</p>}
          {shown.length ? (
            <Card className="gap-0 p-0 shadow-none">
              <ul aria-label="Triggers" className="divide-y">
                {shown.map((trigger) => <TriggerRow key={trigger.name} trigger={trigger} actions={actions} />)}
              </ul>
            </Card>
          ) : (
            <div className="rounded-xl border border-dashed py-12 text-center">
              <h4 className="font-medium">No triggers match “{search.trim()}”</h4>
              <Button variant="outline" className="mt-4" onClick={() => setSearch("")}>Clear search</Button>
            </div>
          )}
        </>
      )}
    </SettingsSection>
  );
}

/** The tail of a trigger's most recent run, re-read while it is open. */
function TriggerLogPane({ source, name }: { source?: string; name: string }) {
  const log = useQuery({
    queryKey: triggerLogKey(source, name),
    queryFn: ({ signal }) => settingsRequest<TriggerLog>(source, "/triggers/log", { name }, signal),
    refetchInterval: source ? 5000 : 2000, retry: false,
  });
  const pane = useRef<HTMLPreElement>(null);
  const pinned = useRef(true);
  const output = log.data?.output ?? "";

  // New output keeps the newest line in view, unless the reader has scrolled up to read.
  useLayoutEffect(() => {
    const element = pane.current;
    if (element && pinned.current) element.scrollTop = element.scrollHeight;
  }, [output]);

  if (log.isPending) return <p role="status" className="text-muted-foreground flex items-center gap-2 py-10 text-sm"><LoaderCircle className="size-4 motion-safe:animate-spin" aria-hidden="true" /> Loading the log…</p>;
  if (log.isError && !log.data) return <div role="alert" className="rounded-xl border p-6"><p>Couldn’t read the log.</p><p className="text-muted-foreground mt-1 text-sm">{log.error.message}</p><Button variant="outline" className="mt-4" onClick={() => void log.refetch()}>Try again</Button></div>;
  if (!log.data.fired) return <p data-slot="trigger-log-empty" className="text-muted-foreground rounded-xl border border-dashed px-6 py-12 text-center text-sm">This trigger has never run — its log is empty.</p>;

  return (
    <div data-slot="trigger-log">
      <p className="text-muted-foreground mb-2 flex flex-wrap gap-x-3 text-xs">
        <span>The most recent output, refreshed while this is open.</span>
        {log.data.fired_at && <span title={fullDate(log.data.fired_at)}>Last wrote {formatAge(log.data.fired_at)}</span>}
        {log.isError && <span role="alert">Couldn’t refresh just now.</span>}
      </p>
      <pre
        ref={pane}
        tabIndex={0}
        aria-label={`Log of ${name}`}
        onScroll={(event) => { const element = event.currentTarget; pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 24; }}
        className="bg-muted/50 focus-visible:ring-ring max-h-[65svh] min-h-32 overflow-auto rounded-lg border p-3 font-mono text-xs leading-relaxed break-words whitespace-pre-wrap outline-none focus-visible:ring-2"
      >
        {output || " "}
      </pre>
    </div>
  );
}

function TriggerDetail({ source, machine, query, name, view, initialPreset, actions }: {
  source?: string; machine: string; query: UseQueryResult<TriggersState>;
  /** The trigger that is open; `null` while a new one is being written. */
  name: string | null; view: "definition" | "log"; initialPreset: string | null; actions: TriggerActions;
}) {
  const navigate = useNavigate();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const trigger = name === null ? undefined : query.data?.triggers.find((item) => item.name === name);
  const back = <Link to={triggerHref()} className="text-muted-foreground hover:text-foreground focus-visible:ring-ring mb-4 inline-flex items-center gap-2 rounded-sm text-sm outline-none focus-visible:ring-2"><ArrowLeft className="size-4" aria-hidden="true" />All triggers</Link>;
  const waiting = { disabled: actions.busy !== null, disabledReason: "Wait for the current action to finish." };

  if (query.isPending) return <>{back}<p role="status" className="text-muted-foreground flex items-center gap-2 py-12 text-sm"><LoaderCircle className="size-4 motion-safe:animate-spin" aria-hidden="true" /> Loading triggers…</p></>;
  if (!query.data) return <>{back}<LoadProblem machine={machine} query={query} /></>;
  if (name !== null && !trigger) {
    return <>{back}<div role="alert" data-slot="trigger-missing" className="rounded-xl border border-dashed p-8 text-center"><h3 className="font-medium">No trigger named “{name}”</h3><p className="text-muted-foreground mt-2 text-sm">It is not defined on {machine}. It may have been deleted or renamed.</p></div></>;
  }

  return (
    <>
      {back}
      <SettingsSection
        data-slot="settings-trigger"
        data-trigger={trigger?.name}
        title={trigger ? <span className="flex flex-wrap items-center gap-x-3 gap-y-2"><span className="font-mono break-all">{trigger.name}</span><StatusBadge trigger={trigger} /></span> : "New trigger"}
        description={trigger ? [triggerStatus(trigger).detail, trigger.last_fired_at ? `last run ${formatAge(trigger.last_fired_at)}` : "never run"].filter(Boolean).join(" · ") : description}
        actions={trigger && (
          <>
            {trigger.running
              ? <Button variant="outline" data-slot="trigger-restart" {...waiting} onClick={() => void actions.restart(trigger.name).then((result) => result && actions.announce(result.message))}><RotateCw className="size-3.5" aria-hidden="true" />Restart</Button>
              : <Button variant="outline" data-slot="trigger-fire" {...waiting} onClick={() => void actions.fire(trigger.name).then((result) => result && actions.announce(result.message))}><Play className="size-3.5" aria-hidden="true" />Fire once</Button>}
            <Button variant="ghost" data-slot="trigger-delete" aria-label={`Delete ${trigger.name}`} {...waiting} onClick={() => { actions.clear(); setConfirmDelete(true); }}><Trash2 className="size-3.5" aria-hidden="true" />Delete…</Button>
          </>
        )}
      >
        {trigger && (
          <nav aria-label="Trigger views" className="mb-6 flex gap-1 border-b">
            {([["definition", "Definition", triggerHref(trigger.name)], ["log", "Log", triggerHref(trigger.name, "log")]] as const).map(([id, label, href]) => (
              <Link key={id} to={href} replace aria-current={view === id ? "page" : undefined} className={cn("focus-visible:ring-ring -mb-px border-b-2 px-3 py-2 text-sm font-medium outline-none focus-visible:ring-2", view === id ? "border-foreground text-foreground" : "text-muted-foreground hover:text-foreground border-transparent")}>{label}</Link>
            ))}
          </nav>
        )}
        {actions.notice && <p role="status" data-slot="trigger-notice" className="bg-muted mb-5 rounded-lg border p-3 text-sm">{actions.notice}</p>}
        {actions.error && !confirmDelete && view === "log" && <p role="alert" className="text-destructive mb-5 rounded-lg border p-3 text-sm">{actions.error}</p>}
        {trigger && view === "log"
          ? <TriggerLogPane source={source} name={trigger.name} />
          : (
            <TriggerEditor
              // A different trigger is a different form; a saved one keeps what was typed.
              key={trigger?.name ?? "new"}
              source={source}
              state={query.data}
              trigger={trigger}
              initialPreset={initialPreset}
              busy={actions.busy?.startsWith("save:") ?? false}
              error={confirmDelete ? null : actions.error}
              onSave={async (body) => {
                const saved = await actions.save(body);
                if (!saved) return false;
                actions.announce(trigger ? `Saved “${body.name}”.` : `Created “${body.name}”.`, !trigger);
                if (!trigger) navigate(triggerHref(body.name), { replace: true });
                return true;
              }}
            />
          )}
      </SettingsSection>
      {confirmDelete && trigger && (
        <ConfirmDialog
          title={`Delete ${trigger.name}`}
          confirmLabel="Delete trigger"
          busy={actions.busy !== null}
          error={actions.error}
          onClose={() => { actions.clear(); setConfirmDelete(false); }}
          onConfirm={async () => {
            if (!(await actions.remove(trigger.name))) return;
            actions.announce(`Deleted “${trigger.name}”.`, true);
            navigate(triggerHref(), { replace: true });
          }}
        >
          <p>Delete the trigger “{trigger.name}” from {machine}? Its definition and code block are removed{trigger.kind === "background" ? ", and its process is stopped" : trigger.kind === "webhook" ? `, and ${webhookPath(trigger.name)} stops answering` : ""}. This cannot be undone.</p>
        </ConfirmDialog>
      )}
    </>
  );
}

function TriggersSection({ source }: { source?: string }) {
  const [params] = useSearchParams();
  const name = params.get("trigger");
  const creating = name === null && params.get("new") === "1";
  const view = params.get("view") === "log" ? "log" : "definition";
  const machine = source ? sourceName(source) : "this machine";
  // A trigger's status moves on its own — a background process comes up, a webhook fires — so
  // the list is re-read while the page is open; a paired machine is asked less often.
  const query = useQuery({ queryKey: triggersKey(source), queryFn: loadTriggers(source), refetchInterval: source ? 15000 : 5000, retry: false });
  const actions = useTriggerActions(source);
  const place = `${name ?? ""}|${creating}|${view}`;
  const first = useRef(true);

  useEffect(() => {
    if (first.current) { first.current = false; return; }
    actions.moved();
  }, [place]);

  if (name !== null || creating) return <TriggerDetail source={source} machine={machine} query={query} name={name} view={view} initialPreset={params.get("preset")} actions={actions} />;
  return <TriggerList machine={machine} query={query} actions={actions} />;
}

export function TriggersSettings() {
  const [source] = useSettingsSource();
  // Another machine has other triggers: its view starts afresh.
  return <TriggersSection key={source ?? ""} source={source} />;
}
