import { Hand, MessageCircle, UserCheck, type LucideIcon } from "lucide-react";
import type { AttentionItem, AttentionKind } from "@/api/board";
import type { BadgeTone } from "@/components/badge/badges";

export interface AttentionSpec {
  /** The badge the card shows for this kind on its own. */
  tone: BadgeTone;
  /** The Needs you mark's icon and text when this kind leads. */
  icon: LucideIcon;
  mark: string;
  /** The tooltip line that names the item. */
  describe: string;
}

const SPECS: Record<AttentionKind, AttentionSpec> = {
  question: {
    icon: MessageCircle,
    tone: "needs-you",
    mark: "Question for you",
    describe: "Question: an agent waits for your answer",
  },
  human_review: {
    icon: UserCheck,
    tone: "human-review",
    mark: "Your review",
    describe: "Human review: a proposal waits for your approval",
  },
  stalled: {
    icon: Hand,
    tone: "stalled",
    mark: "Stalled · needs you",
    describe: "Stalled: its worker made no edits for a while",
  },
  stale_claim: {
    icon: Hand,
    tone: "stale",
    mark: "Stale claim · needs you",
    describe: "Stale claim: the worker's lease ran out",
  },
};

/** The order in which kinds lead the mark: a person's answer or approval first. */
const ORDER: readonly AttentionKind[] = [
  "question",
  "human_review",
  "stalled",
  "stale_claim",
];

export function attentionSpec(kind: AttentionKind): AttentionSpec {
  return SPECS[kind];
}

/** Items are typed loosely because the daemon can be newer than this build and send a kind it does not know. */
type Items = readonly AttentionItem[] | readonly { kind: string }[];

export function hasKind(items: Items, kind: AttentionKind): boolean {
  return items.some((item) => item.kind === kind);
}

export interface AttentionMark {
  icon: LucideIcon;
  text: string;
  /** Names each item on the card. */
  tooltip: string;
}

/** The Needs you mark, or null when the card has no item. A kind this build does not know is ignored. */
export function attentionMark(items: Items): AttentionMark | null {
  const kinds = ORDER.filter((kind) => hasKind(items, kind));
  const [lead] = kinds;
  if (lead === undefined) {
    return null;
  }
  return {
    icon: SPECS[lead].icon,
    text: SPECS[lead].mark,
    tooltip: kinds.map((kind) => SPECS[kind].describe).join(" · "),
  };
}
