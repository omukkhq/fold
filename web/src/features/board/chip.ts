import {
  Activity,
  CirclePause,
  CircleStop,
  ClipboardCheck,
  Clock,
  CloudOff,
  Gauge,
  GitPullRequest,
  Hand,
  Hourglass,
  ListOrdered,
  KeyRound,
  LoaderCircle,
  MessageCircle,
  OctagonAlert,
  UserCheck,
  type LucideIcon,
} from "lucide-react";
import type { BoardCard } from "@/api/board";
import type { ChipTone } from "@/components/badge/chips";
import { planWaitLabel } from "@/features/proposals/planWait";
import type { QueueWaitState } from "@/features/proposals/queueWait";
import type { WorkerActivity } from "@/live/workerActivity";
import { attentionChipText, hasKind } from "./attention";
import {
  blockedText,
  checklistText,
  kindText,
  integrationWait,
  leadCheckMark,
  needsText,
} from "./card";
import { checkRunLabel, type CheckRunView } from "./checkRun";

export interface CardChip {
  tone: ChipTone;
  icon: LucideIcon;
  text: string;
}

type ChipCard = Pick<
  BoardCard,
  | "attention"
  | "blocked"
  | "blocked_by"
  | "held_by"
  | "hold"
  | "kind"
  | "proposal"
  | "report"
  | "status"
>;

/** `T-3, T-5`: the held tasks a task waits on, in the order the daemon sorted them; null for none. */
export function heldThroughText(
  card: Pick<BoardCard, "held_by">,
): string | null {
  const ids = card.held_by ?? [];
  return ids.length === 0 ? null : ids.join(", ");
}

/**
 * The muted chip of a task that waits on held tasks, apart from the one chip `cardChip` picks: the manual hold is
 * gold because it waits for a person, while this calculated hold is only a fact, so it stays neutral. A task with
 * both shows both. A Done task has none.
 */
export function heldChip(
  card: Pick<BoardCard, "held_by" | "status">,
): CardChip | null {
  const ids = heldThroughText(card);
  if (card.status === "done" || ids === null) {
    return null;
  }
  return { tone: "neutral", icon: CirclePause, text: `Held through ${ids}` };
}

/** The words of a slot wait on the card, which leave out the slot's role so the narrow chip keeps one line; the details panel names it. */
export const SLOT_CHIP_TEXT = "Waiting for a free slot";

/** What fold saw of the worker as a chip: gold only once the stall waits for a person, else the icon says what kind of state it is. */
function activityChip(activity: WorkerActivity): CardChip {
  if (activity.needsYou) {
    return { tone: "warn", icon: Hand, text: activity.text };
  }
  if (activity.stall) {
    return { tone: "neutral", icon: Clock, text: activity.text };
  }
  switch (activity.state) {
    case "stopped":
      return { tone: "neutral", icon: CircleStop, text: activity.text };
    case "waits_for_slot":
      return { tone: "neutral", icon: Hourglass, text: SLOT_CHIP_TEXT };
    case "waits_for_network":
      return { tone: "neutral", icon: CloudOff, text: activity.text };
    case "waits_for_usage_limit":
      return { tone: "neutral", icon: Gauge, text: activity.text };
    case "waits_for_login":
      return { tone: "neutral", icon: KeyRound, text: activity.text };
    case "parked":
      return { tone: "neutral", icon: CirclePause, text: activity.text };
    default:
      return { tone: "neutral", icon: Activity, text: activity.text };
  }
}

/**
 * The one chip a card shows, or null. The first state that holds wins: a person must answer or approve, a human
 * hold, an unfinished prerequisite (before the stall, which it causes), a check run that goes, which no person
 * waits for, which the daemon never counts as a stall and during which the worker's claim never runs out, what fold saw
 * of the worker (work in progress and a wait for a lead's go in the neutral tone, a stall in gold only once it waits for
 * a person; a stale claim, which waits for a person, shows over the neutral states), a stall and then a stale claim that
 * only the daemon's items tell of, each in gold with the person's action, a proposal back with its author, then a wait
 * on others. `run` is the card's run and `activity` its worker's
 * activity as the live frame of its workspace has them. A Done card has no chip.
 */
export function cardChip(
  card: ChipCard,
  run: CheckRunView | null = null,
  activity: WorkerActivity | null = null,
): CardChip | null {
  if (card.status === "done") {
    return null;
  }
  if (hasKind(card.attention, "question")) {
    return { tone: "warn", icon: MessageCircle, text: "Question for you" };
  }
  if (hasKind(card.attention, "human_review")) {
    return { tone: "warn", icon: UserCheck, text: "Your review" };
  }
  if (card.hold !== null) {
    return { tone: "warn", icon: Hand, text: "On hold" };
  }
  const blocked = blockedText(card);
  if (blocked !== null) {
    return { tone: "bad", icon: OctagonAlert, text: blocked };
  }
  if (run !== null) {
    return { tone: "neutral", icon: LoaderCircle, text: checkRunLabel(run) };
  }
  const stale: CardChip = {
    tone: "warn",
    icon: Hand,
    text: attentionChipText(card.attention, "stale_claim"),
  };
  if (activity !== null) {
    // A stale claim reaches the card only once it waits for a person, so it shows over what fold saw unless that waits for a person too.
    return hasKind(card.attention, "stale_claim") && !activity.needsYou
      ? stale
      : activityChip(activity);
  }
  if (hasKind(card.attention, "stalled")) {
    return {
      tone: "warn",
      icon: Hand,
      text: attentionChipText(card.attention, "stalled"),
    };
  }
  if (hasKind(card.attention, "stale_claim")) {
    return stale;
  }
  return proposalChip(card);
}

function waitIcon(state: QueueWaitState): LucideIcon {
  switch (state) {
    case "queue":
      return ListOrdered;
    case "person":
      return UserCheck;
    case "waiting":
      return Hourglass;
  }
}

function proposalChip(card: ChipCard): CardChip | null {
  const proposal = card.proposal;
  if (proposal?.state === "conflict") {
    return {
      tone: "bad",
      icon: GitPullRequest,
      text: `${proposal.id} conflict`,
    };
  }
  if (proposal?.state === "changes_requested") {
    return {
      tone: "bad",
      icon: GitPullRequest,
      text: `${proposal.id} changes requested`,
    };
  }
  if (proposal?.state === "open" && proposal.review_claim !== null) {
    return {
      tone: "neutral",
      icon: GitPullRequest,
      text: `${proposal.id} being reviewed`,
    };
  }
  if (proposal?.state === "open") {
    return proposal.review_plan === null
      ? {
          tone: "neutral",
          icon: GitPullRequest,
          text: `${proposal.id} waits for review`,
        }
      : {
          tone: "neutral",
          icon: Hourglass,
          text: `${proposal.id} ${planWaitLabel(proposal.review_plan)}`,
        };
  }
  // The title never depends on the clock; only the detail, which the detail panel shows, names an estimate.
  const wait = proposal ? integrationWait(proposal, 0) : null;
  if (proposal && wait !== null) {
    return {
      tone: wait.state === "person" ? "warn" : "neutral",
      icon: waitIcon(wait.state),
      text: `${proposal.id} ${wait.title}`,
    };
  }
  const check = leadCheckMark(card);
  return check === null
    ? null
    : { tone: "neutral", icon: ClipboardCheck, text: check.text };
}

/** `integrated as r4` for a card whose proposal reached main; null otherwise. */
export function integratedText(
  card: Pick<BoardCard, "proposal">,
): string | null {
  const proposal = card.proposal;
  return proposal?.state === "integrated" && proposal.revision !== null
    ? `integrated as ${proposal.revision}`
    : null;
}

/**
 * The one fact at the end of a card's line, after the id and the labels. A card with a worker names the worker and
 * nothing more; every other fact is in the detail panel. A card without one names the first fact that holds:
 * where a Done task reached main or its checklist, else the prerequisites it waits for, the kind of work, the tasks
 * it blocks, or its checklist.
 */
export function metaFact(
  card: Pick<
    BoardCard,
    | "blocked"
    | "blocked_by"
    | "blocks"
    | "checklist"
    | "kind"
    | "proposal"
    | "status"
    | "worker"
  >,
): string | null {
  if (card.worker !== null) {
    return card.worker;
  }
  const checklist = checklistText(card);
  if (card.status === "done") {
    return (
      integratedText(card) ?? (checklist ? `checklist ${checklist}` : null)
    );
  }
  return (
    needsText(card) ??
    kindText(card) ??
    (card.blocks > 0 ? `blocks ${String(card.blocks)}` : null) ??
    (checklist ? `checklist ${checklist}` : null)
  );
}
