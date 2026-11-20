/*
 * The last result of a read, kept in this browser's localStorage so a page opens with what it
 * showed last time and then refreshes, instead of filling in as each read answers.
 *
 * Whatever is stored is parsed again on the way back in, by the same parser that reads the
 * server's answer. A stored value in an older shape, or one somebody edited, is therefore
 * either understood exactly as a live answer would be or dropped; it is never trusted as it is.
 */

/** A result as it was last read, and when: a query starts from it and still refreshes. */
export interface Saved<T> { data: T; savedAt: number }
/** Saved results of one kind, one per id, of which only the most recently saved are kept. */
export interface SavedFamily { prefix: string; keep: number }

/** One result larger than this is not worth the quota: it is left out, and so is its old copy. */
const maxEntryChars = 400_000;
/** What this page last stored under each key, so a poll that changed nothing writes nothing. */
const written = new WeakMap<Storage, Map<string, string>>();

export function browserStorage(): Storage | undefined {
  try { return window.localStorage; } catch { return undefined; }
}

function fingerprint(body: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < body.length; index++) hash = Math.imul(hash ^ body.charCodeAt(index), 0x01000193);
  return `${body.length}:${hash >>> 0}`;
}

function seenBy(storage: Storage): Map<string, string> {
  let seen = written.get(storage);
  if (!seen) written.set(storage, seen = new Map());
  return seen;
}

export function readSaved<T>(key: string, parse: (payload: unknown) => T, storage = browserStorage()): Saved<T> | undefined {
  try {
    const saved: unknown = JSON.parse(storage?.getItem(key) ?? "null");
    if (typeof saved !== "object" || saved === null) return undefined;
    const { savedAt, payload } = saved as { savedAt?: unknown; payload?: unknown };
    if (typeof savedAt !== "number" || !Number.isFinite(savedAt)) return undefined;
    return { data: parse(payload), savedAt };
  } catch { return undefined; }
}

/** Stores `payload` under `key`. Answers whether anything was written. */
export function writeSaved(key: string, payload: unknown, storage = browserStorage(), now = Date.now()): boolean {
  if (!storage) return false;
  try {
    const body = JSON.stringify(payload);
    if (body === undefined) return false;
    const seen = seenBy(storage);
    const mark = fingerprint(body);
    if (seen.get(key) === mark) return false;
    if (body.length > maxEntryChars) {
      storage.removeItem(key);
      seen.delete(key);
      return false;
    }
    // `savedAt` leads so that pruning can read an entry's age without parsing the entry.
    storage.setItem(key, `{"savedAt":${Math.floor(now)},"payload":${body}}`);
    seen.set(key, mark);
    return true;
  } catch { return false; /* Private mode or a full store: the next visit starts without it. */ }
}

function prune({ prefix, keep }: SavedFamily, storage: Storage) {
  const entries: { key: string; savedAt: number }[] = [];
  for (let index = 0; index < storage.length; index++) {
    const key = storage.key(index);
    if (!key?.startsWith(prefix)) continue;
    const stamp = /^\{"savedAt":(\d+),/.exec(storage.getItem(key)?.slice(0, 40) ?? "");
    entries.push({ key, savedAt: stamp ? Number(stamp[1]) : 0 });
  }
  entries.sort((a, b) => b.savedAt - a.savedAt);
  for (const { key } of entries.slice(keep)) {
    storage.removeItem(key);
    seenBy(storage).delete(key);
  }
}

export function readSavedIn<T>(family: SavedFamily, id: string, parse: (payload: unknown) => T, storage = browserStorage()): Saved<T> | undefined {
  return readSaved(family.prefix + id, parse, storage);
}

/** Stores one member of a family and, the first time this page stores it, drops the family's oldest beyond `keep`. */
export function writeSavedIn(family: SavedFamily, id: string, payload: unknown, storage = browserStorage(), now = Date.now()): boolean {
  if (!storage) return false;
  const key = family.prefix + id;
  const known = seenBy(storage).has(key);
  const stored = writeSaved(key, payload, storage, now);
  if (stored && !known) { try { prune(family, storage); } catch { /* A store that cannot be listed keeps what it has. */ } }
  return stored;
}
