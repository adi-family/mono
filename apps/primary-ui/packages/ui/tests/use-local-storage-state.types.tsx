import { jsonStorageCodec, type LocalStorageCodec, useLocalStorageState } from "../src/components";

type Theme = "light" | "dark";

export function acceptedLocalStorageExamples(storageKey: string | undefined) {
  // The fallback alone fixes the type when the default JSON codec is used.
  const [count, setCount] = useLocalStorageState("count", 0);
  setCount(count + 1);
  setCount((previous) => previous + 1);

  // A codec can widen the type beyond the fallback: `undefined` here means "not chosen".
  const widthCodec: LocalStorageCodec<number | undefined> = {
    parse: (stored) => (Number.isFinite(Number(stored)) ? Number(stored) : undefined),
    serialize: (value) => (value === undefined ? undefined : String(value)),
  };
  const [width, setWidth] = useLocalStorageState(storageKey, undefined, widthCodec);
  setWidth(240);
  setWidth(undefined);
  const widthValue: number | undefined = width;

  const themeCodec: LocalStorageCodec<Theme | null> = {
    parse: (stored) => (stored === "light" || stored === "dark" ? stored : undefined),
    serialize: (value) => value ?? undefined,
  };
  const [theme, setTheme] = useLocalStorageState("theme", null, themeCodec);
  setTheme("dark");
  setTheme(null);
  const themeValue: Theme | null = theme;

  // The JSON codec can be composed with validation.
  const listCodec: LocalStorageCodec<string[]> = {
    ...jsonStorageCodec,
    parse: (stored) => {
      const value = jsonStorageCodec.parse(stored);
      return Array.isArray(value) && value.every((item) => typeof item === "string") ? value : undefined;
    },
  };
  const [list] = useLocalStorageState("list", [], listCodec);
  const listValue: string[] = list;

  return { widthValue, themeValue, listValue };
}

export function rejectedLocalStorageExamples() {
  const [, setCount] = useLocalStorageState("count", 0);
  // @ts-expect-error — the setter only accepts the fallback's type.
  setCount("many");

  // @ts-expect-error — a codec must match the fallback's type.
  useLocalStorageState<number>("width", 0, { parse: (stored) => stored, serialize: (value: string) => value });

  // @ts-expect-error — a fallback is required.
  useLocalStorageState("count");
}
