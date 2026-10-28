import { describe, expect, it } from "vitest";
import {
  THEME_STORAGE_KEY,
  parseStoredTheme,
  readStoredTheme,
  themeAttribute,
  writeStoredTheme,
} from "./theme";

const throwing = {
  getItem: () => {
    throw new Error("blocked");
  },
  setItem: () => {
    throw new Error("blocked");
  },
};

describe("parseStoredTheme", () => {
  it("accepts light, dark and system only", () => {
    expect(parseStoredTheme("light")).toBe("light");
    expect(parseStoredTheme("dark")).toBe("dark");
    expect(parseStoredTheme("system")).toBe("system");
    for (const bad of [null, "", "Dark", "auto", " light"]) {
      expect(parseStoredTheme(bad)).toBeNull();
    }
  });
});

describe("storage helpers", () => {
  it("reads and writes the choice", () => {
    const map = new Map<string, string>();
    const storage = {
      getItem: (k: string) => map.get(k) ?? null,
      setItem: (k: string, v: string) => void map.set(k, v),
    };
    expect(readStoredTheme(storage)).toBeNull();
    expect(writeStoredTheme(storage, "dark")).toBe(true);
    expect(map.get(THEME_STORAGE_KEY)).toBe("dark");
    expect(readStoredTheme(storage)).toBe("dark");
  });
  it("survives a throwing localStorage", () => {
    expect(readStoredTheme(throwing)).toBeNull();
    expect(writeStoredTheme(throwing, "light")).toBe(false);
  });
});

describe("themeAttribute", () => {
  it("leaves the attribute off for system and no choice", () => {
    expect(themeAttribute("dark")).toBe("dark");
    expect(themeAttribute("light")).toBe("light");
    expect(themeAttribute("system")).toBeNull();
    expect(themeAttribute(null)).toBeNull();
  });
});
