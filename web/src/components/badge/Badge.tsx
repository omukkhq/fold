import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { badgeSpec, badgeText, type BadgeTone } from "./badges";

interface BadgeProps {
  tone: BadgeTone;
  /** Replaces the tone's text, for example `Blocked by T-9`. */
  children?: string;
  /** Replaces the tone's icon, for example the attention mark's icon for its leading kind. */
  icon?: LucideIcon;
  /** Content after the text, for example a person's `@name`. */
  after?: ReactNode;
  className?: string;
}

export function Badge({ tone, children, icon, after, className }: BadgeProps) {
  const spec = badgeSpec(tone);
  const Icon = icon ?? spec.icon;
  return (
    <span
      data-tone={tone}
      className={cn(
        "inline-flex items-center gap-6 rounded-pill font-medium whitespace-nowrap",
        spec.shape,
        spec.classes,
        className,
      )}
    >
      <Icon aria-hidden="true" size={14} />
      {badgeText(tone, children)}
      {after}
    </span>
  );
}
