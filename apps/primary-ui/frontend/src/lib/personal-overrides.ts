import { useSyncExternalStore } from "react";

/** Shallow field overrides, scoped to an entity and its owning machine. Never sent to ADI. */
export type PersonalOverrides = Record<string, Record<string, unknown>>;
export const personalOverridesKey = "adi.personal-overrides.v1";
export const entityOverrideKey = (kind: string, name: string, source?: string) => JSON.stringify([source ?? "", kind, name]);

export function parseOverrides(raw: string | null): PersonalOverrides {
  try {
    const value: unknown = JSON.parse(raw ?? "{}");
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    return Object.fromEntries(Object.entries(value).filter(([, fields]) => fields && typeof fields === "object" && !Array.isArray(fields)));
  } catch { return {}; }
}

export function applyOverrides<T extends object>(value: T, overrides: PersonalOverrides, key: string): T {
  return { ...value, ...overrides[key] };
}

export function updateOverride(overrides: PersonalOverrides, key: string, field: string, value: unknown): PersonalOverrides {
  const fields = { ...overrides[key] };
  if (value === undefined) delete fields[field];
  else Object.defineProperty(fields, field, { value, enumerable: true, configurable: true, writable: true });
  const next = { ...overrides };
  if (Object.keys(fields).length) next[key] = fields;
  else delete next[key];
  return next;
}

const changed = "adi-personal-overrides-change";
let cachedRaw: string | null | undefined;
let cached: PersonalOverrides = {};
function snapshot() {
  let raw: string | null;
  try { raw = localStorage.getItem(personalOverridesKey); } catch { return cached; }
  if (raw !== cachedRaw) { cachedRaw = raw; cached = parseOverrides(raw); }
  return cached;
}
function subscribe(callback: () => void) {
  const storage = (event: StorageEvent) => { if (event.key === personalOverridesKey || event.key === null) callback(); };
  window.addEventListener("storage", storage);
  window.addEventListener(changed, callback);
  return () => { window.removeEventListener("storage", storage); window.removeEventListener(changed, callback); };
}
export function usePersonalOverrides() {
  const overrides = useSyncExternalStore(subscribe, snapshot);
  return { overrides, setOverride: (key: string, field: string, value: unknown) => {
    const next = updateOverride(snapshot(), key, field, value);
    // Persist first: a failed write must not pretend the preference was saved.
    localStorage.setItem(personalOverridesKey, JSON.stringify(next));
    window.dispatchEvent(new Event(changed));
  } };
}
