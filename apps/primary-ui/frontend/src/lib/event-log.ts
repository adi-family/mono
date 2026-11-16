import { useSyncExternalStore } from "react";
import { APP_API_BASE } from "./app-api";

/*
 * Background failures — a machine that stopped answering a poll, a transcript refresh that
 * came back malformed — are collected here instead of being pushed into the page as banners.
 * The rail's Event log button shows what arrived; the page keeps showing its content.
 *
 * Browser failures have a tab-local fallback and are also submitted to the shared diagnostic
 * store. The rail polls that store for service/agent failures. Seen state belongs to this tab;
 * reading or dismissing a row never deletes the shared history.
 */

export type AppEventLevel = "error" | "warning" | "info";

export interface AppEventInput {
  level: AppEventLevel;
  /** The machine the event is about; `undefined` for this machine or none in particular. */
  source?: string;
  /** The part of the app that raised it: "Chats", "Fleet", … */
  area: string;
  message: string;
  context?: unknown;
}

export interface AppEvent extends AppEventInput {
  id: string;
  firstSeen: number;
  lastSeen: number;
  /** How many times the same event was reported while it stayed collapsed into this row. */
  count: number;
}

export interface EventLogState {
  /** Newest first. */
  events: AppEvent[];
  /** When the log was last opened; anything newer counts as unread. */
  seenAt: number;
}

export const eventLogStorageKey = "adi.event-log.v1";
export const eventLogLimit = 200;
/** A repeat of the same event within this window bumps its row instead of adding one. */
export const eventCollapseMs = 10 * 60_000;

export const emptyEventLog: EventLogState = { events: [], seenAt: 0 };

const sameEvent = (event: AppEventInput, other: AppEventInput) =>
  event.level === other.level && (event.source ?? "") === (other.source ?? "") && event.area === other.area && event.message === other.message;

/** Pure: the state after one report. Collapses a recent repeat, otherwise prepends and trims. */
export function appendEvent(state: EventLogState, input: AppEventInput, now = Date.now(), id = String(now)): EventLogState {
  const repeat = state.events.find((event) => sameEvent(event, input) && now - event.lastSeen <= eventCollapseMs);
  if (repeat) {
    const bumped = { ...repeat, lastSeen: now, count: repeat.count + 1 };
    return { ...state, events: [bumped, ...state.events.filter((event) => event !== repeat)] };
  }
  const { source, ...rest } = input;
  const event: AppEvent = { ...rest, ...(source ? { source } : {}), id, firstSeen: now, lastSeen: now, count: 1 };
  return { ...state, events: [event, ...state.events].slice(0, eventLogLimit) };
}

export function unreadEvents(state: EventLogState): AppEvent[] {
  return state.events.filter((event) => event.lastSeen > state.seenAt);
}

function isEvent(value: unknown): value is AppEvent {
  if (!value || typeof value !== "object") return false;
  const event = value as Record<string, unknown>;
  return typeof event.id === "string" && (event.level === "error" || event.level === "warning" || event.level === "info") && typeof event.area === "string"
    && typeof event.message === "string" && typeof event.firstSeen === "number" && typeof event.lastSeen === "number"
    && typeof event.count === "number" && (event.source === undefined || typeof event.source === "string");
}

export function parseEventLog(stored: string | null): EventLogState {
  if (!stored) return emptyEventLog;
  try {
    const value: unknown = JSON.parse(stored);
    if (!value || typeof value !== "object" || !Array.isArray((value as EventLogState).events)) return emptyEventLog;
    const seenAt = (value as EventLogState).seenAt;
    return { events: (value as EventLogState).events.filter(isEvent).slice(0, eventLogLimit), seenAt: typeof seenAt === "number" ? seenAt : 0 };
  } catch {
    return emptyEventLog;
  }
}

function readStored(): EventLogState {
  try { return parseEventLog(window.sessionStorage.getItem(eventLogStorageKey)); } catch { return emptyEventLog; }
}

function writeStored(state: EventLogState) {
  try { window.sessionStorage.setItem(eventLogStorageKey, JSON.stringify(state)); } catch { /* The log still lives in memory. */ }
}

let state: EventLogState | null = null;
let sequence = 0;
const listeners = new Set<() => void>();

function current(): EventLogState {
  return state ??= typeof window === "undefined" ? emptyEventLog : readStored();
}

function update(next: EventLogState) {
  if (next === current()) return;
  state = next;
  writeStored(next);
  for (const listener of listeners) listener();
}

/** Record a background failure (or a recovery) without touching the page. Safe outside React. */
export function reportEvent(input: AppEventInput) {
  const now = Date.now();
  const id = `${now}-${++sequence}`;
  update(appendEvent(current(), input, now, id));
  // Keep the local fallback even if the server is down. Never report a reporting failure back
  // through this function: that would create an unbounded request/error loop.
  if (typeof window !== "undefined") void fetch(`${APP_API_BASE}/diagnostics/report`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ ...input, source: "browser", context: { machine: input.source, details: input.context, browser_event_id: id },
      event_key: `browser:${globalThis.crypto?.randomUUID?.() ?? id}` }),
    signal: AbortSignal.timeout(5000),
  }).catch(() => {});
}

export interface DiagnosticRecord {
  id: number; recorded_at: number; level: AppEventLevel; source: string; area: string; message: string; context?: unknown;
}

/** Import by durable id, never incrementing counts because a poll saw the same row again. */
export function mergeDiagnostics(log: EventLogState, records: DiagnosticRecord[]): EventLogState {
  const events = new Map(log.events.map((event) => [event.id, event]));
  for (const record of records) {
    if (record.source === "browser" && record.context && typeof record.context === "object") {
      const localId = (record.context as Record<string, unknown>).browser_event_id;
      if (typeof localId === "string") events.delete(localId);
    }
    const id = `stored:${record.id}`;
    events.set(id, { id, level: record.level, source: record.source, area: record.area, message: record.message,
      context: record.context, firstSeen: record.recorded_at, lastSeen: record.recorded_at, count: 1 });
  }
  return { ...log, events: [...events.values()].sort((a, b) => b.lastSeen - a.lastSeen) };
}

export async function queryDiagnostics(query: { before?: number; source?: string; level?: AppEventLevel; search?: string } = {}): Promise<DiagnosticRecord[]> {
  const response = await fetch(`${APP_API_BASE}/diagnostics/query`, {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ ...query, limit: 100 }),
    signal: AbortSignal.timeout(8000),
  });
  if (!response.ok) throw new Error(`Could not load stored logs (HTTP ${response.status}).`);
  const data = await response.json() as { records?: DiagnosticRecord[] };
  if (!Array.isArray(data.records) || !data.records.every((r) => r && Number.isSafeInteger(r.id)
    && typeof r.recorded_at === "number" && Number.isFinite(r.recorded_at) && r.recorded_at >= 0 && r.recorded_at < 8.64e15
    && ["error", "warning", "info"].includes(r.level) && typeof r.source === "string" && typeof r.area === "string"
    && typeof r.message === "string")) throw new Error("The log service returned an invalid response.");
  return data.records;
}

export async function refreshDiagnostics() {
  const merged = mergeDiagnostics(current(), await queryDiagnostics());
  update({ ...merged, events: merged.events.slice(0, 1000) });
}

export function markEventsSeen() {
  const log = current();
  if (log.events.length === 0 || log.events[0]!.lastSeen <= log.seenAt) return;
  update({ ...log, seenAt: Date.now() });
}

export function clearEvents() {
  if (current().events.length === 0) return;
  update({ events: [], seenAt: Date.now() });
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
};

export function useEventLog(): EventLogState {
  return useSyncExternalStore(subscribe, current, () => emptyEventLog);
}

// Whether the panel is showing. Lives here, not in the rail button, so an empty state elsewhere
// ("Conversations are unavailable") can open the log directly.
let panelOpen = false;
const panelListeners = new Set<() => void>();

export function setEventLogOpen(open: boolean) {
  if (open === panelOpen) return;
  panelOpen = open;
  for (const listener of panelListeners) listener();
}

export function useEventLogOpen(): boolean {
  return useSyncExternalStore((listener) => {
    panelListeners.add(listener);
    return () => { panelListeners.delete(listener); };
  }, () => panelOpen, () => false);
}
