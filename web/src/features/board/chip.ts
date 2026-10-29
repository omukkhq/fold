import {
  ClipboardCheck,
  GitPullRequest,
  Hand,
  Hourglass,
  ListChecks,
  MessageCircle,
  OctagonAlert,
  UserCheck,
  type LucideIcon,
} from "lucide-react";
import type { BoardCard, LiveCheckRun } from "@/api/board";
import type { ChipTone } from "@/components/badge/chips";
import { planWaitLabel } from "@/features/proposals/planWait";
import { hasKind } from "./attention";
import {
  blockedText,
  checklistText,
  kindText,
  leadCheckMark,
  needsText,
} from "./card";

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
  | "blocker"
  | "kind"
  | "proposal"
  | "report"
  | "status"
>;

/**
 * The one chip a card shows, or null. The first state that holds wins: a person must answer or approve, a human
 * blocker, an unfinished prerequisite (before the stall, which it causes), a check run that goes, which no person
 * waits for and which the daemon never counts as a stall, a stalled worker or a stale claim, a proposal back with its
 * author, then a wait on others. A stale claim still shows over a check run: a worker that waits for a long run makes
 * no calls, so its lease can run out while the run goes. `run` is the card's run as the live frame of its workspace
 * has it. A Done card has no chip.
 */
export function cardChip(
  card: ChipCard,
  run: LiveCheckRun | null = null,
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
  if (card.blocker !== null) {
    return { tone: "warn", icon: Hand, text: "Blocker" };
  }
  const blocked = blockedText(card);
  if (blocked !== null) {
    return { tone: "bad", icon: OctagonAlert, text: blocked };
  }
  const staleClaim = hasKind(card.attention, "stale_claim");
  if (run !== null && !staleClaim) {
    return { tone: "neutral", icon: ListChecks, text: "Running checks" };
  }
  if (hasKind(card.attention, "stalled")) {
    return { tone: "warn", icon: Hand, text: "Stalled · needs you" };
  }
  if (staleClaim) {
    return { tone: "warn", icon: Hand, text: "Stale claim · needs you" };
  }
  return proposalChip(card);
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
