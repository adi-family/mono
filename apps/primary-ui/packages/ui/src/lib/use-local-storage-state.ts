import * as React from "react";

import { useStateWithDeps } from "./use-state-with-deps";

/** How a value travels through string-only storage. */
export type LocalStorageCodec<T> = {
  /** Turn the stored string into a value; return `undefined` when it is unusable, to fall back. */
  parse: (stored: string) => T | undefined;
  /** Turn a value into its stored string; return `undefined` to remove the key instead. */
  serialize: (value: T) => string | undefined;
};

/** JSON in both directions; the default. `undefined` removes the key, as JSON cannot hold it. */
export const jsonStorageCodec: LocalStorageCodec<unknown> = {
  parse: (stored) => {
    try { return JSON.parse(stored) as unknown; } catch { return undefined; }
  },
  serialize: (value) => (value === undefined ? undefined : JSON.stringify(value)),
};

/** Plain string in both directions, for keys that store bare text rather than JSON. Empty removes the key. */
export const stringStorageCodec: LocalStorageCodec<string> = {
  parse: (stored) => (stored ? stored : undefined),
  serialize: (value) => (value ? value : undefined),
};

function storageArea(): Storage | undefined {
  try {
    return typeof window !== "undefined" ? window.localStorage : globalThis.localStorage;
  } catch {
    return undefined;
  }
}

function readStorage(key: string): string | null {
  try { return storageArea()?.getItem(key) ?? null; } catch { return null; }
}

function writeStorage(key: string, stored: string | undefined) {
  try {
    const storage = storageArea();
    if (!storage) return;
    if (stored === undefined) storage.removeItem(key);
    else storage.setItem(key, stored);
  } catch { /* The value still lives in state when storage is unavailable (private mode, quota, sandboxed frames). */ }
}

// Mounted hooks in this tab. The browser's `storage` event only reaches other tabs, so a write
// tells the other hooks on the same key here to read again.
type Subscriber = { key: string; sync: () => void };
const subscribers = new Set<Subscriber>();

/** Read one value from `localStorage` through `codec`, returning `fallback` when missing, unusable or unavailable. */
export function readLocalStorage<T>(
  key: string | undefined,
  fallback: T,
  codec: LocalStorageCodec<T> = jsonStorageCodec as LocalStorageCodec<T>,
): T {
  const stored = key === undefined ? null : readStorage(key);
  const parsed = stored === null ? undefined : codec.parse(stored);
  return parsed === undefined ? fallback : parsed;
}

/** Write one value to `localStorage` through `codec` and notify any mounted `useLocalStorageState` hooks on the same key. */
export function writeLocalStorage<T>(
  key: string | undefined,
  value: T,
  codec: LocalStorageCodec<T> = jsonStorageCodec as LocalStorageCodec<T>,
) {
  if (key === undefined) return;
  writeStorage(key, codec.serialize(value));
  for (const subscriber of subscribers) {
    if (subscriber.key === key) subscriber.sync();
  }
}

/**
 * State persisted in `window.localStorage` under `key` — `useState` whose value outlives the page.
 * Starts from what is stored (or `fallback` when nothing usable is), writes every change back,
 * and follows changes from other tabs and from other hooks on the same key in this tab. Without
 * a key it is plain state. Unavailable storage degrades to that too, without throwing.
 */
export function useLocalStorageState<T>(
  key: string | undefined,
  fallback: T,
  codec: LocalStorageCodec<T> = jsonStorageCodec as LocalStorageCodec<T>,
): [T, React.Dispatch<React.SetStateAction<T>>] {
  const read = (): T => readLocalStorage(key, fallback, codec);
  const [value, setValue] = useStateWithDeps(read, [key]);

  // What the last committed render saw, for the setter and the sync listeners (both keyed on `key` alone).
  const latest = React.useRef({ value, read, codec });
  React.useLayoutEffect(() => { latest.current = { value, read, codec }; });
  const self = React.useRef<Subscriber | null>(null);

  React.useEffect(() => {
    if (key === undefined) return;
    const subscriber: Subscriber = { key, sync: () => setValue(latest.current.read()) };
    const onStorage = (event: StorageEvent) => {
      if (event.key === key || event.key === null) subscriber.sync();
    };
    self.current = subscriber;
    subscribers.add(subscriber);
    window.addEventListener("storage", onStorage);
    return () => {
      self.current = null;
      subscribers.delete(subscriber);
      window.removeEventListener("storage", onStorage);
    };
  }, [key, setValue]);

  const setStored = React.useCallback<React.Dispatch<React.SetStateAction<T>>>((update) => {
    const next = typeof update === "function" ? (update as (previous: T) => T)(latest.current.value) : update;
    latest.current.value = next;
    setValue(next);
    if (key === undefined) return;
    writeStorage(key, latest.current.codec.serialize(next));
    for (const subscriber of subscribers) {
      if (subscriber.key === key && subscriber !== self.current) subscriber.sync();
    }
  }, [key, setValue]);

  return [value, setStored];
}
