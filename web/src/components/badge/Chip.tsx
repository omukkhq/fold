import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { COUNT_CLASSES, chipClasses, type ChipTone } from "./chips";

export function Chip({
  tone = "neutral",
  className,
  children,
}: {
  tone?: ChipTone;
  className?: string;
  children: ReactNode;
}) {
  return <span className={cn(chipClasses(tone), className)}>{children}</span>;
}

/** A gold count; the caller names what it counts, because a number alone says nothing to a screen reader. */
export function Count({
  value,
  label,
  className,
}: {
  value: number;
  label?: string;
  className?: string;
}) {
  return (
    <span
      aria-label={label}
      role={label === undefined ? undefined : "status"}
      className={cn(COUNT_CLASSES, className)}
    >
      {value}
    </span>
  );
}
