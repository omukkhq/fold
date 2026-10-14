import { resolveTheme, type Theme, type ThemeChoice } from "../theme/theme";

/** The toggle flips the theme on screen, so a "system" choice becomes the opposite of what the system shows. */
export function toggledTheme(
  choice: ThemeChoice,
  systemPrefersDark: boolean,
): Theme {
  return resolveTheme(choice, systemPrefersDark) === "dark" ? "light" : "dark";
}
