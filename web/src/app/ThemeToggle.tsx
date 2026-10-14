import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { SunMoon } from "lucide-react";
import { useTheme } from "../theme/ThemeProvider";
import { toggledTheme } from "./toggledTheme";

export function ThemeToggle() {
  const { choice, setChoice } = useTheme();
  return (
    <button
      type="button"
      aria-label="Toggle theme"
      onClick={() => {
        const systemDark = window.matchMedia(
          "(prefers-color-scheme: dark)",
        ).matches;
        setChoice(toggledTheme(choice, systemDark));
      }}
      className={cn(
        "flex items-center gap-6 rounded-m px-10 py-7 text-text-muted hover:bg-sunken",
        focusRing,
      )}
    >
      <SunMoon size={15} aria-hidden />
    </button>
  );
}
