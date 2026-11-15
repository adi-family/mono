import { useSyncExternalStore } from "react";

import { routeNode, savePreference } from "./app-chat-sources";

/** A paired node's petname, or `undefined` for this machine. */
export function normalizeMachine(value: unknown): string | undefined {
  return typeof value === "string" ? routeNode(value) : undefined;
}

/**
 * A page's single machine choice — whose marketplace, whose settings — remembered across visits
 * under its own key, like the chat sources are. Unlike the chat list these pages show one
 * machine at a time, so the choice is one petname or this machine.
 */
export function createMachineChoice(storageKey: string) {
  const listeners = new Set<() => void>();
  const read = (): string | undefined => {
    try { return normalizeMachine(JSON.parse(localStorage.getItem(storageKey) ?? "null")); }
    catch { return undefined; }
  };
  let current = read();

  function set(source?: string) {
    current = normalizeMachine(source);
    savePreference(storageKey, current ?? "");
    for (const listener of listeners) listener();
  }

  function subscribe(listener: () => void) {
    listeners.add(listener);
    // Another tab choosing a machine is reflected here as well.
    const onStorage = (event: StorageEvent) => {
      if (event.key !== null && event.key !== storageKey) return;
      current = read();
      listener();
    };
    window.addEventListener("storage", onStorage);
    return () => {
      listeners.delete(listener);
      window.removeEventListener("storage", onStorage);
    };
  }

  function use(): [string | undefined, (source?: string) => void] {
    const source = useSyncExternalStore(subscribe, () => current, () => undefined);
    return [source, set];
  }

  return { read, set, use };
}
