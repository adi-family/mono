import { settingsRequest } from "./settings-api";

// The request helper and the project list are shared by every settings section.
export { loadSettingsProjects, settingsProjectsKey, settingsRequest, type SettingsProject } from "./settings-api";

/*
 * Triggers: code blocks a machine launches on a webhook call, on a platform event, or keeps
 * running in the background. The types mirror the native API's wire shapes (`/api/triggers*`);
 * the kinds, runtimes, presets and event catalog are the server's vocabulary, so the editor
 * offers whatever the machine being edited understands.
 */

export type TriggerOption = { id: string; label: string; hint?: string };
export type TriggerPresetField = { key: string; label: string; hint?: string; default?: string };
export type TriggerPreset = {
  id: string; label: string; description?: string; kind: string; runtime: string; code?: string;
  fields?: TriggerPresetField[]; events?: string[];
};
export type TriggerEventType = { name: string; summary?: string; schema?: unknown; example?: unknown };
export type Trigger = {
  name: string; kind: string; runtime?: string; code?: string; preset?: string | null; description?: string;
  enabled?: boolean; project?: string | null; extra?: Record<string, string>; events?: string[]; trigger_on?: string[];
  created_at: number; updated_at: number; last_fired_at?: number | null;
  running?: boolean; pid?: number | null; uptime_secs?: number | null; restarts?: number;
};
export type TriggersState = {
  triggers: Trigger[]; kinds: TriggerOption[]; runtimes?: TriggerOption[]; presets?: TriggerPreset[]; event_types?: TriggerEventType[];
};
/** `POST /triggers/save` — an upsert keyed by `name`; the server owns the timestamps. */
export type SaveTrigger = {
  name: string; kind: string; runtime: string; code: string; preset: string | null; description: string;
  enabled: boolean; project: string | null; extra: Record<string, string>; events: string[]; trigger_on: string[];
};
export type TriggerRunResult = { message: string; state: TriggersState };
export type TriggerLog = { name: string; fired: boolean; output?: string; fired_at?: number | null };
/** `GET /triggers/relay-key` — the machine's webhook-relay public key, in OpenSSH form. */
export type RelayKey = { public_key: string };

// Every read and write is one machine's; `source` is a paired node's petname, or this machine.
export const triggersKey = (source?: string) => ["settings-triggers", source ?? ""] as const;
export const triggerLogKey = (source: string | undefined, name: string) => ["settings-trigger-log", source ?? "", name] as const;
export const relayKeyKey = (source?: string) => ["settings-relay-key", source ?? ""] as const;
/** A machine whose app predates webhook relays answers 404 here; the editor then offers none. */
export const loadRelayKey = (source?: string) => async ({ signal }: { signal: AbortSignal }) => {
  const key = (await settingsRequest<Partial<RelayKey>>(source, "/triggers/relay-key", undefined, signal)).public_key;
  if (typeof key !== "string" || !key) throw new Error("This machine has no webhook-relay key.");
  return key;
};

export const loadTriggers = (source?: string) => ({ signal }: { signal: AbortSignal }) => settingsRequest<TriggersState>(source, "/triggers", undefined, signal);

/** An unset runtime is a shell block — the same default the server saves. */
export const runtimeOrDefault = (runtime?: string | null) => runtime?.trim() || "sh";

/** A trigger name is its file stem and its webhook URL segment: one path segment. */
export const triggerNamePattern = /^(?!\.{1,2}$)[\p{L}\p{N}._-]+$/u;

export type TriggerStatus = { tone: "running" | "listening" | "stopped" | "disabled"; label: string; detail?: string };

export function formatUptime(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  if (total < 60) return `${total}s`;
  if (total < 3600) return `${Math.floor(total / 60)}m`;
  if (total < 86400) { const minutes = Math.floor((total % 3600) / 60); return `${Math.floor(total / 3600)}h${minutes ? ` ${minutes}m` : ""}`; }
  const hours = Math.floor((total % 86400) / 3600);
  return `${Math.floor(total / 86400)}d${hours ? ` ${hours}h` : ""}`;
}

/** How long ago a Unix-seconds moment was, in the one unit that matters. */
export function formatAge(unixSeconds: number, now = Date.now()): string {
  const seconds = Math.floor(now / 1000) - unixSeconds;
  if (seconds < 45) return "just now";
  if (seconds < 3600) return `${Math.max(1, Math.round(seconds / 60))}m ago`;
  if (seconds < 86400) return `${Math.round(seconds / 3600)}h ago`;
  if (seconds < 30 * 86400) return `${Math.round(seconds / 86400)}d ago`;
  return new Date(unixSeconds * 1000).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

/**
 * A background trigger reports its supervised process — up, for how long, and whether it keeps
 * dying; a webhook or event trigger only reports whether it is armed.
 */
export function triggerStatus(trigger: Trigger): TriggerStatus {
  if (trigger.running) {
    const parts = [
      trigger.pid != null ? `pid ${trigger.pid}` : "",
      trigger.uptime_secs != null ? `up ${formatUptime(trigger.uptime_secs)}` : "",
      trigger.restarts ? `${trigger.restarts} ${trigger.restarts === 1 ? "restart" : "restarts"}` : "",
    ].filter(Boolean);
    return { tone: "running", label: "Running", ...(parts.length ? { detail: parts.join(" · ") } : {}) };
  }
  if (!trigger.enabled) return { tone: "disabled", label: "Disabled" };
  // Enabled but not up: it is either coming up, or its code block cannot start at all.
  if (trigger.kind === "background") return { tone: "stopped", label: "Stopped", detail: "check the log" };
  return { tone: "listening", label: "Listening" };
}

export const webhookPath = (name: string) => `/api/hooks/${encodeURIComponent(name)}`;

/** The list's search: name, description, project, kind and subscribed events. */
export function filterTriggers(triggers: Trigger[], query: string): Trigger[] {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return triggers;
  return triggers.filter((trigger) => {
    const text = [trigger.name, trigger.description, trigger.project, trigger.kind, ...(trigger.events ?? [])].join(" ").toLowerCase();
    return words.every((word) => text.includes(word));
  });
}

/** The form's own shape: the Events box is text, a blank project is global. */
export type TriggerDraft = {
  name: string; kind: string; runtime: string; preset: string | null; project: string; description: string;
  code: string; enabled: boolean; extra: Record<string, string>; events: string; triggerOn: string[];
};

export const emptyDraft: TriggerDraft = {
  name: "", kind: "", runtime: "sh", preset: null, project: "", description: "", code: "", enabled: true, extra: {}, events: "", triggerOn: [],
};

export function draftOf(trigger: Trigger): TriggerDraft {
  return {
    name: trigger.name, kind: trigger.kind, runtime: runtimeOrDefault(trigger.runtime), preset: trigger.preset ?? null,
    project: trigger.project ?? "", description: trigger.description ?? "", code: trigger.code ?? "", enabled: trigger.enabled !== false,
    extra: { ...(trigger.extra ?? {}) }, events: (trigger.events ?? []).join("\n"), triggerOn: [...(trigger.trigger_on ?? [])],
  };
}

/** One pattern per line or comma, trimmed, blanks dropped. */
export function parseEventPatterns(text: string): string[] {
  return text.split(/[\n,]/).map((pattern) => pattern.trim()).filter(Boolean);
}

/** Add a pattern to the Events box unless it is already there; the box comes back one per line. */
export function addEventPattern(text: string, name: string): string {
  const patterns = parseEventPatterns(text);
  return (patterns.includes(name) ? patterns : [...patterns, name]).join("\n");
}

/**
 * Fill a draft from a preset: its kind, runtime, code, suggested event patterns, and the default
 * values of the settings it declares. The name and project are the user's, not the preset's, and
 * a description already written is kept.
 */
export function applyPreset(draft: TriggerDraft, preset: TriggerPreset): TriggerDraft {
  const extra = { ...draft.extra };
  for (const field of preset.fields ?? []) if (field.default && extra[field.key] === undefined) extra[field.key] = field.default;
  return {
    ...draft, kind: preset.kind, runtime: runtimeOrDefault(preset.runtime), code: preset.code ?? "", preset: preset.id,
    events: (preset.events ?? []).join("\n"), description: draft.description.trim() ? draft.description : preset.description ?? "", extra,
  };
}

// ---- webhook relays -----------------------------------------------------------------------

/**
 * The settings a webhook's relay owns. They live among the trigger's other settings, but the
 * editor gives them fields of their own: `relay_endpoint` is written by the machine when it
 * registers the trigger with the relay, and must never become a text box.
 */
export const relaySettingKeys = ["relay", "relay_retention", "relay_endpoint", "licence_code"] as const;
const isRelayKey = (key: string) => (relaySettingKeys as readonly string[]).includes(key);

/** The hosted relays, plus a self-hosted one: its setting is `custom:<url>`. */
export const relayOptions = [
  { value: "", label: "Direct only — no relay" },
  { value: "public-eu", label: "Public (EU)" },
  { value: "public-us", label: "Public (US)" },
  { value: "enterprise-eu", label: "Enterprise (EU)" },
  { value: "enterprise-us", label: "Enterprise (US)" },
  { value: "custom", label: "Self-hosted (custom URL)" },
] as const;

export const isCustomRelay = (relay: string) => relay === "custom" || relay.startsWith("custom:");
/** What the relay picker shows for a `relay` setting. */
export const relayChoice = (relay: string) => (isCustomRelay(relay) ? "custom" : relay);
export const customRelayUrl = (relay: string) => (relay.startsWith("custom:") ? relay.slice(7) : "");

/** The relay's address for a `relay` setting — the same table the machine resolves it with. */
export function resolveRelayUrl(relay: string): string | null {
  const hosted: Record<string, string> = {
    "public-eu": "https://eu.webhooks.withadi.dev", "public-us": "https://us.webhooks.withadi.dev",
    "enterprise-eu": "https://eu.ent.webhooks.withadi.dev", "enterprise-us": "https://us.ent.webhooks.withadi.dev",
  };
  if (hosted[relay]) return hosted[relay];
  return customRelayUrl(relay).trim() || null;
}

/** The address to hand an outside sender, once the machine has registered the trigger with its relay. */
export function relayPublicUrl(extra: Record<string, string>): string | null {
  const endpoint = extra.relay_endpoint?.trim();
  const base = resolveRelayUrl(extra.relay ?? "");
  return endpoint && base ? `${base}/h/${endpoint}` : null;
}

export type SettingField = { key: string; label: string; hint: string; secret?: boolean; custom?: boolean };

/** Setting keys become `ADI_<KEY>` in the code block's environment. */
export const settingKeyPattern = /^[a-z][a-z0-9_]*$/;
export const settingEnvName = (key: string) => `ADI_${key.toUpperCase()}`;

/**
 * The settings inputs for a draft as it stands: a webhook's `secret` (the platform itself reads
 * it to guard the endpoint), whatever the applied preset declares, and anything already set that
 * no preset claims — so editing an old definition never silently drops a value. A relay's
 * settings are not among them: the editor's relay group owns those.
 */
export function settingFields(draft: Pick<TriggerDraft, "kind" | "preset" | "extra">, presets: TriggerPreset[] = []): SettingField[] {
  const fields: SettingField[] = [];
  const push = (field: SettingField) => { if (!fields.some((other) => other.key === field.key)) fields.push(field); };
  if (draft.kind === "webhook") push({ key: "secret", label: "Secret", hint: "Optional. Callers must pass ?secret=… when one is set.", secret: true });
  for (const field of presets.find((preset) => preset.id === draft.preset)?.fields ?? []) push({ key: field.key, label: field.label, hint: field.hint ?? "" });
  // The relay's own settings have their own fields, never this free-form list.
  for (const key of Object.keys(draft.extra).sort()) if (!isRelayKey(key)) push({ key, label: key, hint: `Custom setting, read as $${settingEnvName(key)}.`, custom: true });
  return fields;
}

/** A message for the first problem that would make the save fail or mislead, else `null`. */
export function draftProblem(draft: TriggerDraft): string | null {
  const name = draft.name.trim();
  if (!name) return "A trigger name is required.";
  if (!triggerNamePattern.test(name)) return "Use letters, digits, '.', '-' or '_' in the name: it is also the webhook URL segment.";
  if (!draft.kind.trim()) return "Pick how this trigger launches.";
  if (draft.kind === "event" && parseEventPatterns(draft.events).length === 0) return "An event trigger needs at least one event pattern.";
  return null;
}

/**
 * The save body for a draft. Only the settings the form is offering — and a relay's own — are
 * submitted, trimmed and non-empty, so switching presets never leaks the previous one's settings
 * into the save; the project allowlist applies to event and webhook triggers only.
 */
export function saveBody(draft: TriggerDraft, presets: TriggerPreset[] = []): SaveTrigger {
  const offered = new Set(settingFields(draft, presets).map((field) => field.key));
  const extra: Record<string, string> = {};
  // The relay's settings ride along whatever the form offers: dropping `relay_endpoint` on a
  // save would cut a registered webhook off from its relay.
  for (const [key, value] of Object.entries(draft.extra)) if ((offered.has(key) || isRelayKey(key)) && value.trim()) extra[key] = value.trim();
  const kind = draft.kind.trim();
  return {
    name: draft.name.trim(), kind, runtime: runtimeOrDefault(draft.runtime), code: draft.code, preset: draft.preset,
    description: draft.description.trim(), enabled: draft.enabled, project: draft.project.trim() || null, extra,
    events: kind === "event" ? parseEventPatterns(draft.events) : [],
    trigger_on: kind === "event" || kind === "webhook" ? draft.triggerOn : [],
  };
}

/**
 * Flip a trigger's enabled flag by re-saving its whole definition, untouched otherwise. For a
 * background trigger this is its power switch: the supervisor starts or stops the process.
 */
export function enabledBody(trigger: Trigger, enabled: boolean): SaveTrigger {
  return {
    name: trigger.name, kind: trigger.kind, runtime: trigger.runtime ?? "", code: trigger.code ?? "", preset: trigger.preset ?? null,
    description: trigger.description ?? "", enabled, project: trigger.project ?? null, extra: { ...(trigger.extra ?? {}) },
    events: [...(trigger.events ?? [])], trigger_on: [...(trigger.trigger_on ?? [])],
  };
}

/** What the code block is told about how it will be launched. */
export function codeHint(kind: string, runtime: string): string {
  const how = runtimeOrDefault(runtime) === "ts" ? "Run with bun" : "Run as sh -c";
  const payload = kind === "webhook" ? "the request body lands in $ADI_PAYLOAD_FILE, its headers in $ADI_HEADERS_FILE"
    : kind === "event" ? "the event lands in $ADI_PAYLOAD, its name in $ADI_EVENT"
      : "keep it running — the app restarts it if it exits";
  return `${how}; ${payload}; settings arrive as $ADI_<KEY>.`;
}

/** Where a trigger's views live. The name travels in the query, so any valid name survives a reload. */
export function triggerHref(name?: string, view?: "log"): string {
  if (!name) return "/settings/triggers";
  return `/settings/triggers?${new URLSearchParams({ trigger: name, ...(view ? { view } : {}) })}`;
}
export const newTriggerHref = "/settings/triggers?new=1";
