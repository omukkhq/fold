import {
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleSlash,
  GitMerge,
  UserCheck,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/cn";
import { ActorText } from "@/people/ActorText";
import { PILL_CLASSES, type Pill, type PillIcon } from "./present";

const ICONS: Record<PillIcon, LucideIcon> = {
  conflict: GitMerge,
  changes: CircleAlert,
  human: UserCheck,
  approved: CircleCheck,
  waiting: CircleDashed,
  integrated: GitMerge,
  closed: CircleSlash,
};

export function ReviewPill({
  pill,
  large,
  className,
}: {
  pill: Pill;
  /** The detail header draws a larger pill than the list. */
  large?: boolean;
  className?: string;
}) {
  const Icon = ICONS[pill.icon];
  return (
    <span
      data-tone={pill.tone}
      className={cn(
        "inline-flex items-center gap-6 rounded-pill leading-tight font-medium",
        large ? "px-12 py-4 text-14" : "px-9 py-2 text-13",
        PILL_CLASSES[pill.tone],
        className,
      )}
    >
      <Icon aria-hidden="true" size={14} className="shrink-0" />
      <span className="min-w-0">
        {pill.segments ? (
          <ActorText segments={pill.segments} slot="waitingChip" />
        ) : (
          pill.text
        )}
      </span>
    </span>
  );
}
