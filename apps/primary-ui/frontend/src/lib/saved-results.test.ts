/// <reference types="node" />
import { test } from "node:test";
import assert from "node:assert/strict";
import { readSaved, readSavedIn, writeSaved, writeSavedIn, type SavedFamily } from "./saved-results";

function memoryStorage(): Storage & { writes: string[] } {
  const items = new Map<string, string>();
  const writes: string[] = [];
  return {
    writes,
    get length() { return items.size; },
    clear: () => items.clear(),
    getItem: (key) => items.get(key) ?? null,
    key: (index) => [...items.keys()][index] ?? null,
    removeItem: (key) => { items.delete(key); },
    setItem: (key, value) => { writes.push(key); items.set(key, String(value)); },
  };
}

const words = (payload: unknown) => {
  if (!Array.isArray(payload) || !payload.every((item) => typeof item === "string")) throw new Error("not a list of words");
  return payload as string[];
};

test("a saved result comes back through its parser with the time it was saved", () => {
  const storage = memoryStorage();
  assert.equal(readSaved("words", words, storage), undefined);
  assert.equal(writeSaved("words", ["one", "two"], storage, 1000.7), true);
  assert.deepEqual(readSaved("words", words, storage), { data: ["one", "two"], savedAt: 1000 });
  assert.deepEqual(readSaved("words", (payload) => words(payload).length, storage), { data: 2, savedAt: 1000 });
});

test("a read that changed nothing writes nothing, and a change is written once", () => {
  const storage = memoryStorage();
  writeSaved("words", ["one"], storage, 1000);
  assert.equal(writeSaved("words", ["one"], storage, 2000), false);
  assert.equal(readSaved("words", words, storage)?.savedAt, 1000);
  assert.equal(writeSaved("words", ["one", "two"], storage, 3000), true);
  assert.equal(writeSaved("words", ["one", "two"], storage, 4000), false);
  assert.deepEqual(readSaved("words", words, storage), { data: ["one", "two"], savedAt: 3000 });
  assert.deepEqual(storage.writes, ["words", "words"]);
  // Another store has its own history: the same value is new to it.
  assert.equal(writeSaved("words", ["one", "two"], memoryStorage(), 5000), true);
});

test("anything the parser does not accept is an empty start, not an error", () => {
  const storage = memoryStorage();
  for (const value of ["", "{", "null", "[]", "7", "{}", JSON.stringify({ savedAt: "now", payload: ["one"] }), JSON.stringify({ savedAt: 1000 }),
    JSON.stringify({ savedAt: 1000, payload: { words: ["one"] } }), JSON.stringify({ savedAt: 1000, catalog: ["one"] })]) {
    storage.setItem("words", value);
    assert.equal(readSaved("words", words, storage), undefined, value);
  }
});

test("a store that refuses, or is not there, costs nothing but the saved copy", () => {
  const refusing = { ...memoryStorage(), getItem: () => { throw new Error("denied"); }, setItem: () => { throw new Error("quota"); } } as unknown as Storage;
  assert.equal(writeSaved("words", ["one"], refusing), false);
  assert.equal(readSaved("words", words, refusing), undefined);
  assert.equal(writeSavedIn({ prefix: "w:", keep: 2 }, "a", ["one"], refusing), false);
  assert.equal(writeSaved("words", ["one"], undefined), false);
  assert.equal(readSaved("words", words, undefined), undefined);
  assert.equal(readSavedIn({ prefix: "w:", keep: 2 }, "a", words, undefined), undefined);
  const circular: Record<string, unknown> = {};
  circular.self = circular;
  assert.equal(writeSaved("words", circular, memoryStorage()), false);
  assert.equal(writeSaved("words", undefined, memoryStorage()), false);
});

test("a result too large to be worth the quota is left out, along with its older copy", () => {
  const storage = memoryStorage();
  writeSaved("words", ["small"], storage, 1000);
  assert.equal(writeSaved("words", ["x".repeat(400_001)], storage, 2000), false);
  assert.equal(storage.getItem("words"), null);
  assert.equal(writeSaved("words", ["small"], storage, 3000), true, "the smaller value is stored again afterwards");
});

test("a family keeps its most recently saved members, counting those left by earlier visits", () => {
  const family: SavedFamily = { prefix: "thread:", keep: 3 };
  const storage = memoryStorage();
  // Left behind by an earlier page: this page has never written them.
  storage.setItem("thread:old-1", `{"savedAt":100,"payload":["old one"]}`);
  storage.setItem("thread:old-2", `{"savedAt":200,"payload":["old two"]}`);
  storage.setItem("thread:unstamped", `["no stamp"]`);
  storage.setItem("other:kept", `{"savedAt":1,"payload":["another kind"]}`);
  writeSavedIn(family, "a", ["a"], storage, 1000);
  assert.deepEqual([...Array(storage.length).keys()].map((index) => storage.key(index)).sort(), ["other:kept", "thread:a", "thread:old-1", "thread:old-2"]);
  writeSavedIn(family, "b", ["b"], storage, 2000);
  assert.equal(storage.getItem("thread:old-1"), null);
  writeSavedIn(family, "c", ["c"], storage, 3000);
  writeSavedIn(family, "a", ["a", "again"], storage, 4000);
  writeSavedIn(family, "d", ["d"], storage, 5000);
  assert.deepEqual(readSavedIn(family, "a", words, storage), { data: ["a", "again"], savedAt: 4000 });
  assert.equal(readSavedIn(family, "b", words, storage), undefined, "the member saved longest ago gave way");
  assert.deepEqual(readSavedIn(family, "c", words, storage)?.data, ["c"]);
  assert.deepEqual(readSavedIn(family, "d", words, storage)?.data, ["d"]);
  assert.equal(storage.getItem("other:kept") !== null, true);
  // A member that was dropped is stored again when it comes back.
  assert.equal(writeSavedIn(family, "b", ["b"], storage, 6000), true);
  assert.deepEqual(readSavedIn(family, "b", words, storage)?.data, ["b"]);
  assert.equal(readSavedIn(family, "c", words, storage), undefined);
});
