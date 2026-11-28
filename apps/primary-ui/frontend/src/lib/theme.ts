import { useEffect, useLayoutEffect, useState } from "react";

import {
  type LocalStorageCodec,
  readLocalStorage,
  type Theme,
  useLocalStorageState,
} from "@design-system/ui";

const storageKey = "design-system-theme";
const systemQuery = "(prefers-color-scheme: dark)";

const themeCodec: LocalStorageCodec<Theme | null> = {
  parse: (stored) => (stored === "light" || stored === "dark" ? stored : undefined),
  serialize: (value) => value ?? undefined,
};

function getSystemTheme(): Theme {
  return window.matchMedia(systemQuery).matches ? "dark" : "light";
}

function applyTheme(theme: Theme) {
  document.documentElement.classList.toggle("dark", theme === "dark");
}

// Apply the preference before React mounts, without changing the generated HTML shell.
applyTheme(readLocalStorage(storageKey, null, themeCodec) ?? getSystemTheme());

export function useTheme() {
  const [preference, setPreference] = useLocalStorageState<Theme | null>(
    storageKey,
    null,
    themeCodec,
  );
  const [systemTheme, setSystemTheme] = useState(getSystemTheme);
  const theme = preference ?? systemTheme;

  useLayoutEffect(() => {
    applyTheme(theme);
  }, [theme]);

  useEffect(() => {
    const media = window.matchMedia(systemQuery);
    const updateSystemTheme = () => setSystemTheme(media.matches ? "dark" : "light");

    media.addEventListener("change", updateSystemTheme);
    updateSystemTheme();

    return () => {
      media.removeEventListener("change", updateSystemTheme);
    };
  }, []);

  function setTheme(nextTheme: Theme) {
    setPreference(nextTheme);
  }

  return { theme, setTheme };
}
