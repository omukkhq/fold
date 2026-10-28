import { Monitor, Moon, Sun, type LucideIcon } from "lucide-react";
import {
  SEGMENT_LIST,
  segmentClasses,
} from "../components/segments/segmentStyles";
import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { useTheme } from "../theme/ThemeProvider";
import type { ThemeChoice as Choice } from "../theme/theme";

const CHOICES: readonly { value: Choice; label: string; icon: LucideIcon }[] = [
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
  { value: "system", label: "System", icon: Monitor },
];

/** Light, Dark or System; System follows the operating system's setting. */
export function ThemeChoice() {
  const { choice, setChoice } = useTheme();
  return (
    <div
      role="group"
      aria-label="Theme"
      className={cn(SEGMENT_LIST, "flex-nowrap")}
    >
      {CHOICES.map(({ value, label, icon: Icon }) => (
        <button
          key={value}
          type="button"
          aria-pressed={choice === value}
          onClick={() => {
            setChoice(value);
          }}
          className={cn(
            segmentClasses(choice === value),
            "inline-flex min-h-32 flex-1 items-center justify-center gap-6 px-4",
            focusRing,
          )}
        >
          <Icon aria-hidden="true" size={14} />
          {label}
        </button>
      ))}
    </div>
  );
}
