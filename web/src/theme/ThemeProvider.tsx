import { createContext, useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import {
  readStoredTheme,
  themeAttribute,
  writeStoredTheme,
  type ThemeChoice,
} from "./theme";

interface ThemeContextValue {
  choice: ThemeChoice;
  setChoice: (choice: ThemeChoice) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [choice, setChoice] = useState<ThemeChoice>(() => {
    const s = storage();
    return (s && readStoredTheme(s)) ?? "system";
  });

  useEffect(() => {
    const attr = themeAttribute(choice);
    if (attr) {
      document.documentElement.dataset.theme = attr;
    } else {
      delete document.documentElement.dataset.theme;
    }
  }, [choice]);

  const value = useMemo<ThemeContextValue>(
    () => ({
      choice,
      setChoice: (next) => {
        const s = storage();
        if (s) {
          writeStoredTheme(s, next);
        }
        setChoice(next);
      },
    }),
    [choice],
  );

  return <ThemeContext value={value}>{children}</ThemeContext>;
}

export function useTheme(): ThemeContextValue {
  const value = useContext(ThemeContext);
  if (!value) {
    throw new Error("useTheme needs a ThemeProvider");
  }
  return value;
}
