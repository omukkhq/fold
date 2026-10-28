import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { SunMoon } from "lucide-react";
import { useTheme } from "../theme/ThemeProvider";
import { toggledTheme } from "./toggledTheme";

/** `row` is the labelled full-width row of the top bar's More popover; elsewhere the switch is an icon button. */
export function ThemeToggle({
  variant = "icon",
}: {
  variant?: "icon" | "row";
}) {
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
        variant === "row"
          ? "flex min-h-44 items-center gap-8 rounded-m px-14 text-14 text-text hover:bg-sunken"
          : "flex shrink-0 items-center gap-6 rounded-m px-10 py-7 text-text-2 hover:bg-sunken max-lg:size-44 max-lg:justify-center max-lg:p-0",
        focusRing,
      )}
    >
      <SunMoon size={15} aria-hidden />
      {variant === "row" ? "Toggle theme" : null}
    </button>
  );
}
