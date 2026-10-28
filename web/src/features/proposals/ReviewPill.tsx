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
import { ActorText } from "@/people/ActorText";
import { isHumanName } from "@/people/names";
import { PersonName } from "@/people/PersonName";
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
        "inline-flex items-center gap-6 rounded-pill leading-tight font-medium",
        large ? "px-12 py-4 text-14" : "px-9 py-2 text-13",
        PILL_CLASSES[pill.tone],
        className,
      )}
    >
      <Icon aria-hidden="true" size={14} className="shrink-0" />
      {pill.segments ? (
        <ActorText segments={pill.segments} slot="waitingChip" />
      ) : (
        pill.text
      )}
    </span>
  );
}

export function WaitingChip({ waiting }: { waiting: Waiting }) {
  const { actor } = waiting;
  const human = actor === null || isHumanName(actor);
  const Icon = human ? User : Bot;
  return (
    <span className="inline-flex items-center gap-5 rounded-s bg-sunken px-8 py-2 text-13 leading-tight text-text-2">
      <Icon
        role="img"
        aria-label={human ? "human" : "agent"}
        size={12}
        className="shrink-0"
      />
      {actor === null ? (
        <span className="font-semibold">a human</span>
      ) : (
        <PersonName name={actor} slot="waitingChip" />
      )}
    </span>
  );
}
