import { useSyncExternalStore } from "react";

import { routeNode, savePreference } from "./app-chat-sources";

/**
 * Which machine's marketplace is being browsed. A catalog, its sources and its installs are
 * one machine's, so unlike the chat list this is a single choice: a paired node's petname, or
 * `undefined` for this machine. Remembered across visits like the chat sources are.
 */
const storageKey = "adi-marketplace-source";
const listeners = new Set<() => void>();

export function normalizeMarketplaceSource(value: unknown): string | undefined {
  return typeof value === "string" ? routeNode(value) : undefined;
}

export function readMarketplaceSource(): string | undefined {
  try { return normalizeMarketplaceSource(JSON.parse(localStorage.getItem(storageKey) ?? "null")); }
  catch { return undefined; }
}

let current = readMarketplaceSource();

export function setMarketplaceSource(source?: string) {
  current = normalizeMarketplaceSource(source);
  savePreference(storageKey, current ?? "");
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  // Another tab choosing a machine is reflected here as well.
  const onStorage = (event: StorageEvent) => {
    if (event.key !== null && event.key !== storageKey) return;
    current = readMarketplaceSource();
    listener();
  };
  window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("storage", onStorage);
  };
}

export function useMarketplaceSource(): [string | undefined, (source?: string) => void] {
  const source = useSyncExternalStore(subscribe, () => current, () => undefined);
  return [source, setMarketplaceSource];
}
