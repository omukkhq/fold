import { cn } from "@/lib/cn";
import { badgeSpec, badgeText, type BadgeTone } from "./badges";

interface BadgeProps {
  tone: BadgeTone;
  /** Replaces the tone's text, for example `Blocked by T-9`. */
  children?: string;
  className?: string;
}

export function Badge({ tone, children, className }: BadgeProps) {
  const spec = badgeSpec(tone);
  const Icon = spec.icon;
  return (
    <span
      data-tone={tone}
      className={cn(
        "inline-flex items-center gap-5 rounded-s font-semibold",
        spec.shape,
        spec.classes,
        className,
      )}
    >
      <Icon aria-hidden="true" size={12} />
      {badgeText(tone, children)}
    </span>
  );
}
