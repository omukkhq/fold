import type {
  HumanReviewMark,
  CardProposalState,
  TaskStatus,
} from "@/api/board";
import type {
  CheckpointSummary,
  FeedbackRecipient,
  ReviewVerdict,
  TaskClaim,
  TaskEvent,
  TaskIntegration,
} from "@/api/tasks";
import { formatSince } from "@/format/time";
import { atName, reviewerActor } from "@/people/names";
import { actorSegment, seg, type Segment } from "@/people/segments";

export interface StatusLook {
  text: string;
  /** The dot, from the `status-*` token. */
  dot: string;
  /** The pill's fill and text colour. */
  pill: string;
}

// Full class names, so the Tailwind scanner sees every one.
const STATUS: Record<TaskStatus, StatusLook> = {
  backlog: {
    text: "Backlog",
    dot: "bg-status-backlog",
    pill: "bg-sunken text-text-muted",
  },
  todo: { text: "Todo", dot: "bg-status-todo", pill: "bg-sunken text-text" },
  in_progress: {
    text: "In Progress",
    dot: "bg-status-progress",
    pill: "bg-primary-soft text-primary-strong",
  },
  review: {
    text: "Review",
    dot: "bg-status-review",
    pill: "bg-secondary-soft text-secondary-text",
  },
  done: { text: "Done", dot: "bg-status-done", pill: "bg-ink-soft text-text" },
};

export function statusLook(status: TaskStatus): StatusLook {
  return STATUS[status];
}

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

/** The local time of day, `13:40`. */
export function clockTime(at: number): string {
  const when = new Date(at);
  return `${pad(when.getHours())}:${pad(when.getMinutes())}`;
}

/** `Claimed 13:40 · lease renewed 2 min ago`, or the stale lease when the daemon reports it. */
export function claimNote(claim: TaskClaim, now: number): string {
  const lease = claim.stale
    ? `lease stale, last renewed ${formatSince(now, claim.renewed_at)}`
    : `lease renewed ${formatSince(now, claim.renewed_at)}`;
  return `Claimed ${clockTime(claim.claimed_at)} · ${lease}`;
}

const EVENT_VERBS: Partial<Record<TaskEvent["type"], string>> = {
  "task.created": "Created by",
  "task.updated": "Edited by",
  "task.completed": "Completed by",
  "task.human_review_set": "Human review turned on by",
  "task.human_review_cleared": "Human review turned off by",
  "dependency.added": "Dependency added by",
  "dependency.removed": "Dependency removed by",
  "checklist.item_added": "Checklist item added by",
  "checklist.item_updated": "Checklist item changed by",
  "checklist.item_removed": "Checklist item removed by",
  "claim.acquired": "Claimed by",
  "claim.released": "Released by",
  "claim.taken_over": "Taken over by",
  "workspace.created": "Workspace opened by",
  "checkpoint.recorded": "Checkpoint by",
  "changeset.recorded": "Changeset by",
  "proposal.opened": "Proposal opened by",
  "integration.succeeded": "Integrated by",
  "task.reported": "Reported finished by",
  "task.sent_back": "Sent back by",
  "task.hold_set": "Blocker set by",
  "task.hold_cleared": "Blocker cleared by",
};

/** `Claimed by frontend/ui · 13:40`; a type without a sentence shows its name. */
export function historyText(event: TaskEvent): Segment[] {
  const verb = EVENT_VERBS[event.type] ?? `${event.type} by`;
  return seg`${verb} ${actorSegment(event.actor)} · ${clockTime(event.created_at)}`;
}

export interface Collapsed<T> {
  shown: T[];
  /** `Show 2 more`; null when nothing is hidden. */
  more: string | null;
}

/** The first `keep` entries, and the toggle text for the rest. */
export function collapse<T>(
  items: readonly T[],
  keep: number,
  open: boolean,
): Collapsed<T> {
  const hidden = items.length - keep;
  if (hidden <= 0) {
    return { shown: [...items], more: null };
  }
  return open
    ? { shown: [...items], more: "Show fewer" }
    : { shown: items.slice(0, keep), more: `Show ${String(hidden)} more` };
}

/** `auto · quiet 2 min` for an automatic checkpoint, the label for an explicit one. */
export function checkpointText(ck: CheckpointSummary): string {
  if (ck.automatic) {
    return ck.label ? `auto · ${ck.label}` : "auto";
  }
  return ck.label ?? "checkpoint";
}

/** The human-review mark: who must approve, and who set it, with the time when the daemon says. */
export function humanReviewLines(
  mark: HumanReviewMark,
  setAt: number | null,
): { reviewer: string | null; setBy: string; at: string | null } {
  return {
    reviewer: reviewerActor(mark.reviewer),
    setBy: mark.set_by,
    at: setAt === null ? null : clockTime(setAt),
  };
}

const VERDICTS: Record<ReviewVerdict, string> = {
  comment: "commented",
  request_changes: "requested changes",
  approve: "approved",
  reject: "rejected",
};

export function reviewText(review: {
  reviewer: string;
  verdict: ReviewVerdict;
  version: number;
}): Segment[] {
  return seg`${actorSegment(review.reviewer)} ${VERDICTS[review.verdict]} on v${String(review.version)}`;
}

const PROPOSAL_STATES: Record<CardProposalState, string> = {
  open: "waiting for review",
  approved: "approved",
  changes_requested: "changes requested",
  conflict: "conflict",
  integrated: "integrated",
};

export function proposalStateText(state: CardProposalState, version: number) {
  return `${PROPOSAL_STATES[state]} · v${String(version)}`;
}

export function integrationText(integration: TaskIntegration): Segment[] {
  return seg`by ${actorSegment(integration.by)} · ${clockTime(integration.at)}`;
}

/** `2 linked to T-12`. */
export function messageCountText(count: number, task: string): string {
  return `${String(count)} linked to ${task}`;
}

/** The feedback box's default reader: the claim holder, or nobody when the task has no holder. */
export function defaultRecipient(
  holder: string | null,
): FeedbackRecipient | null {
  return holder === null ? null : { kind: "holder" };
}

/** The `<select>` value of a recipient; agent and tag names can be equal, so the kind is part of it. */
export function recipientValue(to: FeedbackRecipient): string {
  return to.kind === "holder" ? "holder" : `${to.kind}:${to.name}`;
}

export function parseRecipient(value: string): FeedbackRecipient | null {
  if (value === "holder") {
    return { kind: "holder" };
  }
  const colon = value.indexOf(":");
  const kind = value.slice(0, colon);
  const name = value.slice(colon + 1);
  if (colon <= 0 || name === "") {
    return null;
  }
  if (kind === "agent" || kind === "tag") {
    return { kind, name };
  }
  return null;
}

/** The placeholder names the reader, `Write feedback to frontend/ui…`. */
export function feedbackPlaceholder(
  to: FeedbackRecipient | null,
  holder: string | null,
): string {
  if (to === null) {
    return "Choose who reads the feedback";
  }
  if (to.kind === "holder") {
    return `Write feedback to ${holder === null ? "the claim holder" : atName(holder)}…`;
  }
  return to.kind === "tag"
    ? `Write feedback to every agent tagged ${to.name}…`
    : `Write feedback to ${to.name}…`;
}
