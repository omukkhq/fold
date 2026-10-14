import { cn } from "@/lib/cn";
import { labelColors } from "./labelColors";

interface LabelProps {
  /** The label text. */
  children: string;
  /** One of the eight colour names; anything else shows as `stone`. */
  color: string;
  className?: string;
}

export function Label({ children, color, className }: LabelProps) {
  const colors = labelColors(color);
  return (
    <span
      className={cn(
        "inline-flex items-center gap-5 rounded-s py-2 pr-7 pl-6 text-label leading-snug font-medium text-text-muted inset-ring inset-ring-border",
        colors.soft,
        className,
      )}
    >
      <span
        aria-hidden="true"
        className={cn("size-6 rounded-full", colors.dot)}
      />
      {children}
    </span>
  );
}
