import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import * as React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { useLocalStorageState } from "../src/components";

// A stand-in for window.localStorage; the test runner has no DOM. Every access can be made to
// throw, as it does in sandboxed frames and some private modes.
function fakeStorage(initial = {}, { broken = false } = {}) {
  const data = new Map(Object.entries(initial));
  const guard = () => { if (broken) throw new Error("SecurityError"); };
  return {
    data,
    getItem(key) { guard(); return data.has(key) ? data.get(key) : null; },
    setItem(key, value) { guard(); data.set(key, String(value)); },
    removeItem(key) { guard(); data.delete(key); },
  };
}

function install(storage) {
  globalThis.window = { localStorage: storage };
  return storage;
}

beforeEach(() => { delete globalThis.window; });
afterEach(() => { delete globalThis.window; });

function Show({ storageKey, fallback, codec }) {
  const [value] = useLocalStorageState(storageKey, fallback, codec);
  return <span>{JSON.stringify(value)}</span>;
}

// Static markup escapes the quotes JSON.stringify produces; undo that so expectations read plainly.
const shown = (element) => renderToStaticMarkup(element).replaceAll("&quot;", '"');

// A render-phase update lets one server render exercise the setter: React re-runs the component
// with the new value, and the storage write happens on the way.
function Edit({ storageKey, fallback, codec, update }) {
  const [value, setValue] = useLocalStorageState(storageKey, fallback, codec);
  const [done, setDone] = React.useState(false);
  if (!done) { setValue(update); setDone(true); }
  return <span>{JSON.stringify(value)}</span>;
}

describe("useLocalStorageState", () => {
  test("reads JSON from storage and falls back when nothing usable is stored", () => {
    install(fakeStorage({ number: "320", object: '{"a":1}', broken: "{not json", nothing: "null" }));
    expect(shown(<Show storageKey="number" fallback={0} />)).toBe("<span>320</span>");
    expect(shown(<Show storageKey="object" fallback={{}} />)).toBe('<span>{"a":1}</span>');
    expect(shown(<Show storageKey="broken" fallback="default" />)).toBe('<span>"default"</span>');
    expect(shown(<Show storageKey="missing" fallback="default" />)).toBe('<span>"default"</span>');
    expect(shown(<Show storageKey="nothing" fallback="default" />)).toBe("<span>null</span>");
  });

  test("applies a custom codec, treating undefined from parse as unusable", () => {
    install(fakeStorage({ width: " 900 ", bad: "wide" }));
    const codec = {
      parse: (stored) => { const n = Number(stored); return Number.isFinite(n) ? Math.min(480, n) : undefined; },
      serialize: (value) => (value === undefined ? undefined : String(value)),
    };
    expect(shown(<Show storageKey="width" fallback={undefined} codec={codec} />)).toBe("<span>480</span>");
    expect(shown(<Show storageKey="bad" fallback={200} codec={codec} />)).toBe("<span>200</span>");
  });

  test("is plain state without a key or without storage", () => {
    expect(shown(<Show fallback="default" />)).toBe('<span>"default"</span>');
    const storage = install(fakeStorage({ k: '"stored"' }, { broken: true }));
    expect(shown(<Show storageKey="k" fallback="default" />)).toBe('<span>"default"</span>');
    expect(shown(<Edit storageKey="k" fallback="default" update="edited" />)).toBe('<span>"edited"</span>');
    expect(storage.data.get("k")).toBe('"stored"');
    const untouched = install(fakeStorage());
    expect(shown(<Edit fallback="default" update="edited" />)).toBe('<span>"edited"</span>');
    expect(untouched.data.size).toBe(0);
  });

  test("writes changes back through the codec and removes the key for undefined", () => {
    const storage = install(fakeStorage({ count: "1" }));
    expect(shown(<Edit storageKey="count" fallback={0} update={(previous) => previous + 1} />)).toBe("<span>2</span>");
    expect(storage.data.get("count")).toBe("2");

    expect(shown(<Edit storageKey="name" fallback="" update="Ada" />)).toBe('<span>"Ada"</span>');
    expect(storage.data.get("name")).toBe('"Ada"');

    expect(shown(<Edit storageKey="count" fallback={0} update={undefined} />)).toBe("<span></span>");
    expect(storage.data.has("count")).toBe(false);

    const codec = { parse: (stored) => stored, serialize: (value) => (value ? value.toUpperCase() : undefined) };
    expect(shown(<Edit storageKey="theme" fallback="" codec={codec} update="dark" />)).toBe('<span>"dark"</span>');
    expect(storage.data.get("theme")).toBe("DARK");
    expect(shown(<Edit storageKey="theme" fallback="" codec={codec} update="" />)).toBe('<span>""</span>');
    expect(storage.data.has("theme")).toBe(false);
  });
});
