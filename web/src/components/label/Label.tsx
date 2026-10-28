import { cn } from "@/lib/cn";
import { labelColors } from "./labelColors";

interface LabelProps {
  /** The label text. */
  children: string;
  /** One of the eight colour names; anything else shows as `stone`. */
  color: string;
  className?: string;
}

/** An area label as a coloured dot before its name: no chip and no outline, so a card holds one less box. */
export function Label({ children, color, className }: LabelProps) {
  const colors = labelColors(color);
  return (
    <span
      className={cn(
        "inline-flex items-center gap-6 text-13 leading-tight whitespace-nowrap text-text-2",
        className,
      )}
    >
      <span
        aria-hidden="true"
        className={cn("size-8 shrink-0 rounded-full", colors.dot)}
      />
      {children}
    </span>
  );
}
