import { Hand, MessageCircle, UserCheck, type LucideIcon } from "lucide-react";
import type { AttentionItem, AttentionKind, PersonAction } from "@/api/board";

interface AttentionSpec {
  /** The Needs you mark's icon and text when this kind leads. */
  icon: LucideIcon;
  mark: string;
  /** The tooltip line that names the item. */
  describe: string;
}

const SPECS: Record<AttentionKind, AttentionSpec> = {
  question: {
    icon: MessageCircle,
    mark: "Question for you",
    describe: "Question: an agent waits for your answer",
  },
  human_review: {
    icon: UserCheck,
    mark: "Your review",
    describe: "Human review: a proposal waits for your approval",
  },
  stalled: {
    icon: Hand,
    mark: "Stalled",
    describe:
      "Stalled: fold told the worker's lead and it did not start again, or no agent can act",
  },
  stale_claim: {
    icon: Hand,
    mark: "Stale claim",
    describe:
      "Stale claim: the worker made no fold call for its lease, and its lead did not act or no agent can",
  },
};

/** What the person does, as the mark says it after the kind and the tooltip after its line. */
const ACTIONS: Record<PersonAction, { mark: string; describe: string }> = {
  restart_worker: {
    mark: "restart worker",
    describe: "restart the worker in a new session",
  },
  message_worker: {
    mark: "message worker",
    describe: "message the worker, which resumes it in its session",
  },
};

/** The order in which kinds lead the mark: a person's answer or approval first. */
const ORDER: readonly AttentionKind[] = [
  "question",
  "human_review",
  "stalled",
  "stale_claim",
];

/** Items are typed loosely because the daemon can be newer than this build and send a kind or an action it does not know. */
type Item = Pick<AttentionItem, "kind"> & { action?: string | null };
type Items =
  | readonly AttentionItem[]
  | readonly Item[]
  | readonly { kind: string }[];

export function hasKind(items: Items, kind: AttentionKind): boolean {
  return items.some((item) => item.kind === kind);
}

/** The action the daemon names for the first item of `kind`; null for a kind without one, or an action this build does not know. */
export function actionOf(
  items: Items,
  kind: AttentionKind,
): PersonAction | null {
  const item = (items as readonly Item[]).find((each) => each.kind === kind);
  const action = item?.action ?? null;
  return action === "restart_worker" || action === "message_worker"
    ? action
    : null;
}

export interface AttentionMark {
  icon: LucideIcon;
  text: string;
  /** Names each item on the card, with what the person does about a stall or a stale claim. */
  tooltip: string;
}

/** `Stale claim · message worker`: a stall or a stale claim names the person's action, and says `needs you` only when the daemon names none. */
function markText(items: Items, kind: AttentionKind): string {
  const spec = SPECS[kind];
  if (kind !== "stalled" && kind !== "stale_claim") {
    return spec.mark;
  }
  const action = actionOf(items, kind);
  return `${spec.mark} · ${action === null ? "needs you" : ACTIONS[action].mark}`;
}

function describe(items: Items, kind: AttentionKind): string {
  const action = actionOf(items, kind);
  return action === null
    ? SPECS[kind].describe
    : `${SPECS[kind].describe}; ${ACTIONS[action].describe}`;
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
    text: markText(items, lead),
    tooltip: kinds.map((kind) => describe(items, kind)).join(" · "),
  };
}

/** The chip of a stall or a stale claim that only the daemon's item tells of: `Stale claim · message worker`. */
export function attentionChipText(
  items: Items,
  kind: "stalled" | "stale_claim",
): string {
  return markText(items, kind);
}
