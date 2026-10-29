export type Theme = "light" | "dark";
export type ThemeChoice = Theme | "system";

export const THEME_STORAGE_KEY = "fold.theme";

export function parseStoredTheme(raw: string | null): ThemeChoice | null {
  return raw === "light" || raw === "dark" || raw === "system" ? raw : null;
}

/** Storage can be missing or throw (private windows, blocked site data); the page then follows the system. */
export function readStoredTheme(
  storage: Pick<Storage, "getItem">,
): ThemeChoice | null {
  try {
    return parseStoredTheme(storage.getItem(THEME_STORAGE_KEY));
  } catch {
    return null;
  }
}

export function writeStoredTheme(
  storage: Pick<Storage, "setItem">,
  choice: ThemeChoice,
): boolean {
  try {
    storage.setItem(THEME_STORAGE_KEY, choice);
    return true;
  } catch {
    return false;
  }
}

/** "system" leaves the attribute off so the stylesheet's media query decides. */
export function themeAttribute(choice: ThemeChoice | null): Theme | null {
  return choice === "light" || choice === "dark" ? choice : null;
}
