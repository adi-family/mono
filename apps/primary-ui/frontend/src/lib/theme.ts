import { useEffect, useLayoutEffect, useState } from "react";

import type { Theme } from "@design-system/ui";

const storageKey = "design-system-theme";
const systemQuery = "(prefers-color-scheme: dark)";

function parseTheme(value: string | null): Theme | null {
  return value === "light" || value === "dark" ? value : null;
}

function readPreference(): Theme | null {
  try {
    return parseTheme(window.localStorage.getItem(storageKey));
  } catch {
    // Theme switching still works when browser storage is unavailable.
    return null;
  }
}

function getSystemTheme(): Theme {
  return window.matchMedia(systemQuery).matches ? "dark" : "light";
}

function applyTheme(theme: Theme) {
  document.documentElement.classList.toggle("dark", theme === "dark");
}

// Apply the preference before React mounts, without changing the generated HTML shell.
applyTheme(readPreference() ?? getSystemTheme());

export function useTheme() {
  const [preference, setPreference] = useState(readPreference);
  const [systemTheme, setSystemTheme] = useState(getSystemTheme);
  const theme = preference ?? systemTheme;

  useLayoutEffect(() => {
    applyTheme(theme);
  }, [theme]);

  useEffect(() => {
    const media = window.matchMedia(systemQuery);
    const updateSystemTheme = () => setSystemTheme(media.matches ? "dark" : "light");
    const syncPreference = (event: StorageEvent) => {
      try {
        if (
          event.storageArea === window.localStorage &&
          (event.key === storageKey || event.key === null)
        ) {
          setPreference(parseTheme(event.newValue));
        }
      } catch {
        // Retain the in-memory preference if access to storage has been revoked.
      }
    };

    media.addEventListener("change", updateSystemTheme);
    window.addEventListener("storage", syncPreference);
    updateSystemTheme();

    return () => {
      media.removeEventListener("change", updateSystemTheme);
      window.removeEventListener("storage", syncPreference);
    };
  }, []);

  function setTheme(nextTheme: Theme) {
    setPreference(nextTheme);
    try {
      window.localStorage.setItem(storageKey, nextTheme);
    } catch {
      // Keep the user's choice for this session even if it cannot be persisted.
    }
  }

  return { theme, setTheme };
}
