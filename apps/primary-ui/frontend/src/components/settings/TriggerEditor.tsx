import { useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { Check, Copy, Eye, EyeOff, LoaderCircle, Plus } from "lucide-react";
import { Link } from "react-router-dom";

import { Button, CodeEditor, Field, Input, Select, Switch, Textarea, cn } from "@design-system/ui";

import { copyText } from "../../lib/copy-text";
import {
  addEventPattern, applyPreset, codeHint, customRelayUrl, draftOf, draftProblem, emptyDraft, isCustomRelay, loadRelayKey, loadSettingsProjects,
  relayChoice, relayKeyKey, relayOptions, relayPublicUrl, relaySettingKeys, runtimeOrDefault, saveBody, settingEnvName, settingFields,
  settingKeyPattern, settingsProjectsKey, triggerHref, webhookPath,
  type SaveTrigger, type SettingField, type Trigger, type TriggerDraft, type TriggersState,
} from "../../lib/triggers";

function Group({ title, hint, children }: { title: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <fieldset className="min-w-0 space-y-3">
      <legend className="text-sm font-medium">{title}</legend>
      {hint && <p className="text-muted-foreground -mt-1 text-xs leading-relaxed">{hint}</p>}
      {children}
    </fieldset>
  );
}

/** One named setting. A webhook's secret is hidden until asked for. */
function SettingInput({ field, value, onChange }: { field: SettingField; value: string; onChange: (value: string) => void }) {
  const [shown, setShown] = useState(false);
  return (
    <Field label={field.label} hint={field.hint || undefined}>
      {(control) => (
        <div className="flex items-center gap-1.5">
          <Input {...control} mono type={field.secret && !shown ? "password" : "text"} autoComplete="off" spellCheck={false} data-setting={field.key} value={value} onChange={(event) => onChange(event.target.value)} />
          {field.secret && (
            <Button type="button" variant="ghost" size="icon" aria-label={shown ? "Hide the secret" : "Show the secret"} aria-pressed={shown} onClick={() => setShown((current) => !current)}>
              {shown ? <EyeOff className="size-4" aria-hidden="true" /> : <Eye className="size-4" aria-hidden="true" />}
            </Button>
          )}
        </div>
      )}
    </Field>
  );
}

/** A value to hand to someone else: shown whole, never editable, one click to copy. */
function CopyRow({ label, hint, value, slot }: { label: string; hint?: ReactNode; value: string; slot: string }) {
  const [copied, setCopied] = useState<boolean | null>(null);
  return (
    <Field label={label} hint={copied === false ? "Couldn’t reach the clipboard. Select the text and copy it by hand." : hint} className="sm:col-span-2">
      {(control) => (
        <div className="flex items-center gap-1.5">
          <Input {...control} mono readOnly data-slot={slot} value={value} onFocus={(event) => event.currentTarget.select()} />
          <Button type="button" variant="outline" size="sm" aria-label={`Copy ${label.toLowerCase()}`} onClick={() => void copyText(value).then(setCopied)}>
            {copied ? <Check className="size-3.5" aria-hidden="true" /> : <Copy className="size-3.5" aria-hidden="true" />}
            {copied ? "Copied" : "Copy"}
          </Button>
        </div>
      )}
    </Field>
  );
}

/**
 * A webhook's relay: lets an outside sender reach the trigger without the machine being exposed.
 * The machine registers the trigger with the chosen relay when it is saved and writes back the
 * endpoint, which this group only ever shows.
 */
function RelayGroup({ extra, relayKey, onChange }: { extra: Record<string, string>; relayKey: string | undefined; onChange: (extra: Record<string, string>) => void }) {
  const relay = extra.relay ?? "";
  const publicUrl = relayPublicUrl(extra);
  const put = (key: string, value: string) => onChange({ ...extra, [key]: value });
  return (
    <Group title="Webhook relay" hint="Lets an outside sender — Stripe, GitHub — reach this trigger without exposing the machine. Direct only keeps it reachable on this machine’s own address alone.">
      <div data-slot="trigger-relay" className="grid gap-x-5 gap-y-5 sm:grid-cols-2">
        <Field label="Relay">
          {(control) => <Select {...control} name="relay" options={relayOptions} value={relayChoice(relay)} onValueChange={(choice) => put("relay", choice === "custom" ? `custom:${customRelayUrl(relay)}` : choice)} />}
        </Field>
        {isCustomRelay(relay) && (
          <Field label="Relay URL" hint="Your own self-hosted relay.">
            {(control) => <Input {...control} mono name="relay_url" autoComplete="off" spellCheck={false} placeholder="https://relay.example.com" value={customRelayUrl(relay)} onChange={(event) => put("relay", `custom:${event.target.value}`)} />}
          </Field>
        )}
        {relay && (
          <Field label="Retention (seconds)" hint="How long the relay keeps a delivery the machine has not collected. Blank uses the relay’s default.">
            {(control) => <Input {...control} mono name="relay_retention" type="number" min={0} inputMode="numeric" value={extra.relay_retention ?? ""} onChange={(event) => put("relay_retention", event.target.value)} />}
          </Field>
        )}
        {relay.startsWith("enterprise-") && (
          <Field label="Licence code" hint="Checked once, at registration, against this install’s ADI Enterprise licence.">
            {(control) => <Input {...control} mono name="licence_code" autoComplete="off" spellCheck={false} value={extra.licence_code ?? ""} onChange={(event) => put("licence_code", event.target.value)} />}
          </Field>
        )}
        {publicUrl && <CopyRow label="Public URL" hint="Give this to the sender." value={publicUrl} slot="trigger-relay-url" />}
        {relay && !publicUrl && <p data-slot="trigger-relay-pending" className="text-muted-foreground text-sm sm:col-span-2">The public URL appears here once the trigger is saved and the machine has registered it with the relay.</p>}
        {relay && relayKey && <CopyRow label="This machine’s public key" hint="For a self-hosted relay’s authorized_keys." value={relayKey} slot="trigger-relay-key" />}
      </div>
    </Group>
  );
}

/**
 * The definition form for a new or existing trigger. The draft is taken once, when the form
 * opens, so the list refreshing in the background never rewrites what is being typed.
 */
export function TriggerEditor({ source, state, trigger, initialPreset, busy, error, onSave }: {
  source?: string;
  state: TriggersState;
  /** The trigger being edited; absent for a new one. */
  trigger?: Trigger;
  initialPreset?: string | null;
  busy: boolean;
  error: string | null;
  onSave: (body: SaveTrigger) => Promise<boolean>;
}) {
  const presets = state.presets ?? [];
  const [draft, setDraft] = useState<TriggerDraft>(() => {
    if (trigger) return draftOf(trigger);
    const preset = presets.find((item) => item.id === initialPreset);
    return preset ? applyPreset(emptyDraft, preset) : emptyDraft;
  });
  const [problem, setProblem] = useState<string | null>(null);
  const [newKey, setNewKey] = useState("");
  const [newKeyProblem, setNewKeyProblem] = useState<string | null>(null);
  const projects = useQuery({ queryKey: settingsProjectsKey(source), queryFn: loadSettingsProjects(source), staleTime: 30000, retry: false });
  // Only a machine whose app knows webhook relays answers with its key; one that does not is
  // offered no relay, unless the definition already carries one.
  const relayKey = useQuery({ queryKey: relayKeyKey(source), queryFn: loadRelayKey(source), enabled: draft.kind === "webhook", staleTime: Infinity, retry: false });
  const relayed = draft.kind === "webhook" && (relayKey.isSuccess || relaySettingKeys.some((key) => !!draft.extra[key]));
  const set = (patch: Partial<TriggerDraft>) => { setProblem(null); setDraft((current) => ({ ...current, ...patch })); };

  const kinds = state.kinds.map((kind) => ({ value: kind.id, label: kind.label }));
  const runtimes = (state.runtimes ?? []).map((runtime) => ({ value: runtime.id, label: runtime.label }));
  if (!runtimes.some((runtime) => runtime.value === draft.runtime)) runtimes.push({ value: draft.runtime, label: draft.runtime });
  const known = projects.data ?? [];
  // A trigger filed under a project that is archived, or not listed yet, keeps its project.
  const projectOptions = [
    { value: "", label: "Global — not filed under a project" },
    ...known.map((project) => ({ value: project.id, label: project.name })),
    ...(draft.project && !known.some((project) => project.id === draft.project) ? [{ value: draft.project, label: draft.project }] : []),
  ];
  const restrictable = draft.kind === "event" || draft.kind === "webhook";
  const allowlist = [...known.map((project) => ({ id: project.id, name: project.name })), ...draft.triggerOn.filter((id) => !known.some((project) => project.id === id)).map((id) => ({ id, name: id }))];
  const fields = settingFields(draft, presets);
  const body = saveBody(draft, presets);
  const changed = !trigger || JSON.stringify(body) !== JSON.stringify(saveBody(draftOf(trigger), presets));
  const eventTypes = state.event_types ?? [];

  function addSetting() {
    const key = newKey.trim().toLowerCase();
    if (!settingKeyPattern.test(key)) { setNewKeyProblem("Use lowercase letters, digits and underscores, starting with a letter."); return; }
    if (fields.some((field) => field.key === key)) { setNewKeyProblem(`“${key}” is already one of this trigger's settings.`); return; }
    set({ extra: { ...draft.extra, [key]: "" } });
    setNewKey("");
    setNewKeyProblem(null);
  }

  return (
    <form
      data-slot="trigger-form"
      aria-busy={busy}
      // The form reports its own problems, in one place and in words that name the fix.
      noValidate
      className="space-y-8"
      onSubmit={async (event) => {
        event.preventDefault();
        const found = draftProblem(draft);
        setProblem(found);
        if (!found) await onSave(body);
      }}
    >
      {presets.length > 0 && (
        <Group title="Start from a preset" hint="Fills the form with a working code block. Edit it however you like.">
          <div className="flex flex-wrap gap-1.5">
            {presets.map((preset) => (
              <Button key={preset.id} type="button" variant="outline" size="sm" data-preset={preset.id} title={preset.description} aria-pressed={draft.preset === preset.id} className={cn(draft.preset === preset.id && "bg-accent text-accent-foreground border-foreground/30")} onClick={() => set(applyPreset(draft, preset))}>
                {preset.label}
              </Button>
            ))}
          </div>
        </Group>
      )}

      <div className="grid gap-x-5 gap-y-5 sm:grid-cols-2">
        <Field label="Name" hint={trigger ? "The name is the trigger's file, so it cannot be changed." : draft.kind === "webhook" && draft.name.trim() ? <>Also the webhook URL segment: <code className="font-mono">{webhookPath(draft.name.trim())}</code></> : "Letters, digits, '.', '-' or '_'. Also the webhook URL segment."}>
          {(control) => <Input {...control} mono name="name" required readOnly={!!trigger} autoComplete="off" spellCheck={false} placeholder="deploy-hook" value={draft.name} onChange={(event) => set({ name: event.target.value })} />}
        </Field>
        <Field label="Description" hint="One line, shown in the list.">
          {(control) => <Input {...control} name="description" autoComplete="off" placeholder="What this trigger does" value={draft.description} onChange={(event) => set({ description: event.target.value })} />}
        </Field>
        <Field label="Launches" hint={state.kinds.find((kind) => kind.id === draft.kind)?.hint || "How the code block gets launched."}>
          {(control) => <Select {...control} name="kind" required options={kinds} placeholder="How does it launch?" value={draft.kind} onValueChange={(kind) => set({ kind })} />}
        </Field>
        <Field label="Language" hint={(state.runtimes ?? []).find((runtime) => runtime.id === draft.runtime)?.hint}>
          {(control) => <Select {...control} name="runtime" options={runtimes} value={draft.runtime} onValueChange={(runtime) => set({ runtime })} />}
        </Field>
        <Field label="Project" hint={projects.isError ? "Couldn’t load the projects; the current choice is kept." : "Files the trigger on that project's page. It does not limit what the trigger may do."}>
          {(control) => <Select {...control} name="project" options={projectOptions} value={draft.project} onValueChange={(project) => set({ project })} />}
        </Field>
        <div data-slot="field" className="flex min-w-0 flex-col gap-1.5">
          <span id="trigger-enabled-label" className="text-sm leading-none font-medium">Enabled</span>
          <div className="flex min-h-8 items-center gap-3">
            <Switch name="enabled" aria-labelledby="trigger-enabled-label" aria-describedby="trigger-enabled-hint" checked={draft.enabled} onCheckedChange={(enabled) => set({ enabled })} />
            <span className="text-sm">{draft.enabled ? "On" : "Off"}</span>
          </div>
          <p id="trigger-enabled-hint" className="text-muted-foreground text-xs leading-relaxed">
            {draft.kind === "background" ? "The app keeps an enabled background trigger running, and stops it when disabled." : "A disabled trigger keeps its definition and can still be fired by hand."}
          </p>
        </div>
      </div>

      {restrictable && (
        <Group title="Run only for projects" hint="None checked means every project. Otherwise it fires only when the payload names a checked project.">
          {allowlist.length ? (
            <div data-slot="trigger-allowlist" className="flex flex-wrap gap-x-5 gap-y-2">
              {allowlist.map((project) => (
                <label key={project.id} className="flex items-center gap-2 text-sm">
                  <input type="checkbox" className="accent-primary size-4" checked={draft.triggerOn.includes(project.id)} onChange={(event) => set({ triggerOn: event.target.checked ? [...draft.triggerOn, project.id] : draft.triggerOn.filter((id) => id !== project.id) })} />
                  {project.name}
                </label>
              ))}
            </div>
          ) : <p className="text-muted-foreground text-sm">{projects.isPending ? "Loading projects…" : "There are no projects on this machine yet."}</p>}
        </Group>
      )}

      <Group title="Settings" hint={<>Named values the code block reads from its environment — a setting called <code className="font-mono">interval_secs</code> arrives as <code className="font-mono">${settingEnvName("interval_secs")}</code>. Clear a value to remove it.</>}>
        {fields.length > 0 && (
          <div data-slot="trigger-settings" className="grid gap-x-5 gap-y-5 sm:grid-cols-2">
            {fields.map((field) => <SettingInput key={field.key} field={field} value={draft.extra[field.key] ?? ""} onChange={(value) => set({ extra: { ...draft.extra, [field.key]: value } })} />)}
          </div>
        )}
        <div className="flex max-w-md flex-wrap items-start gap-2">
          <div className="min-w-40 flex-1">
            <Input mono size="sm" aria-label="New setting name" aria-invalid={newKeyProblem ? true : undefined} placeholder="setting_name" autoComplete="off" spellCheck={false} value={newKey}
              onChange={(event) => { setNewKey(event.target.value); setNewKeyProblem(null); }}
              onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); addSetting(); } }} />
          </div>
          <Button type="button" variant="outline" size="sm" data-slot="trigger-add-setting" onClick={addSetting}><Plus className="size-3.5" aria-hidden="true" />Add setting</Button>
          {newKeyProblem && <p role="alert" className="text-destructive basis-full text-xs">{newKeyProblem}</p>}
        </div>
      </Group>

      {relayed && <RelayGroup extra={draft.extra} relayKey={relayKey.data} onChange={(extra) => set({ extra })} />}

      {draft.kind === "event" && (
        <Group title="Events">
          <Field label="Patterns" hint={<>One per line. <code className="font-mono">*</code> matches one segment, <code className="font-mono">**</code> the rest — <code className="font-mono">adi.tasks.*</code></>}>
            {(control) => <Textarea {...control} mono name="events" rows={3} spellCheck={false} placeholder="adi.tasks.*" value={draft.events} onChange={(event) => set({ events: event.target.value })} />}
          </Field>
          {eventTypes.length > 0 && (
            <div>
              <p className="text-muted-foreground mb-2 text-xs">Available events — choose one to add it. Each arrives in <code className="font-mono">$ADI_PAYLOAD</code>, its name in <code className="font-mono">$ADI_EVENT</code>.</p>
              <div data-slot="trigger-event-catalog" className="flex flex-wrap gap-1.5">
                {eventTypes.map((type) => {
                  const example = type.example === undefined || type.example === null ? "" : JSON.stringify(type.example);
                  return (
                    <button key={type.name} type="button" title={[type.summary, example && `Example: ${example}`].filter(Boolean).join("\n")} className="bg-muted hover:bg-accent hover:text-accent-foreground focus-visible:ring-ring rounded-md px-2 py-1 font-mono text-xs outline-none focus-visible:ring-2" onClick={() => set({ events: addEventPattern(draft.events, type.name) })}>
                      {type.name}
                    </button>
                  );
                })}
              </div>
            </div>
          )}
        </Group>
      )}

      <Field label="Code block" hint={codeHint(draft.kind, draft.runtime)}>
        {/* Highlighted as the language the runtime picker says, so the block reads the way its interpreter will read it. */}
        {(control) => <CodeEditor {...control} name="code" language={runtimeOrDefault(draft.runtime) === "ts" ? "ts" : "sh"} rows={18} value={draft.code} onValueChange={(code) => set({ code })} />}
      </Field>

      <div className="flex flex-wrap items-center gap-3 border-t pt-5">
        <Button type="submit" data-slot="trigger-save" disabled={busy || !changed} disabledReason={busy ? "The trigger is being saved." : "There are no changes to save."}>
          {busy && <LoaderCircle className="size-4 motion-safe:animate-spin" aria-hidden="true" />}
          {busy ? "Saving…" : trigger ? "Save changes" : "Create trigger"}
        </Button>
        <Button asChild variant="ghost"><Link to={triggerHref()}>{trigger && !changed ? "Back to triggers" : "Cancel"}</Link></Button>
        {(problem ?? error) && <p role="alert" data-slot="trigger-form-error" className="text-destructive min-w-0 basis-full text-sm sm:basis-auto">{problem ?? error}</p>}
      </div>
    </form>
  );
}
