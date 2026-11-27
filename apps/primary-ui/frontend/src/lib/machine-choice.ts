import { useCallback } from "react";

import {
  jsonStorageCodec,
  type LocalStorageCodec,
  readLocalStorage,
  useLocalStorageState,
  writeLocalStorage,
} from "@design-system/ui";

import { routeNode } from "./app-chat-sources";

/** A paired node's petname, or `undefined` for this machine. */
export function normalizeMachine(value: unknown): string | undefined {
  return typeof value === "string" ? routeNode(value) : undefined;
}

const machineCodec: LocalStorageCodec<string | undefined> = {
  parse: (stored) => normalizeMachine(jsonStorageCodec.parse(stored)),
  serialize: (value) => JSON.stringify(normalizeMachine(value) ?? ""),
};

/**
 * A page's single machine choice — whose marketplace, whose settings — remembered across visits
 * under its own key, like the chat sources are. Unlike the chat list these pages show one
 * machine at a time, so the choice is one petname or this machine.
 */
export function createMachineChoice(storageKey: string) {
  const read = (): string | undefined => readLocalStorage(storageKey, undefined, machineCodec);
  const set = (source?: string) => writeLocalStorage(storageKey, normalizeMachine(source), machineCodec);

  function use(): [string | undefined, (source?: string) => void] {
    const [source, setStored] = useLocalStorageState<string | undefined>(storageKey, undefined, machineCodec);
    const choose = useCallback((next?: string) => setStored(normalizeMachine(next)), [setStored]);
    return [source, choose];
  }

  return { read, set, use };
}
