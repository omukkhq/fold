import {
  Bot,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleSlash,
  GitMerge,
  User,
  UserCheck,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/cn";
import {
  PILL_CLASSES,
  type Pill,
  type PillIcon,
  type Waiting,
} from "./present";

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
        "inline-flex items-center rounded-s text-meta leading-snug font-semibold",
        large ? "gap-6 px-9 py-3" : "gap-5 px-8 py-2",
        PILL_CLASSES[pill.tone],
        className,
      )}
    >
      <Icon aria-hidden="true" size={large ? 13 : 12} className="shrink-0" />
      {pill.text}
    </span>
  );
}

export function WaitingChip({ waiting }: { waiting: Waiting }) {
  const Icon = waiting.human ? User : Bot;
  return (
    <span className="inline-flex items-center gap-5 rounded-s bg-sunken px-8 py-2 text-meta leading-snug font-semibold text-text-muted">
      <Icon
        role="img"
        aria-label={waiting.human ? "human" : "agent"}
        size={12}
        className="shrink-0"
      />
      {waiting.name}
    </span>
  );
}
