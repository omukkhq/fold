import type {
  ComparisonTotals,
  HumanReviewMark,
  ProposalDetail,
  ProposalSummary,
  ProposalVersion,
  Review,
  ReviewScopeRule,
  Verdict,
} from "@/api/proposals";
import type { WorkerList } from "@/api/workers";
import { bareName, isHumanName, reviewerActor } from "@/people/names";
import {
  actorSegment,
  reviewerSegment,
  seg,
  segmentsText,
  type Segments,
} from "@/people/segments";

export type PillTone = "danger" | "human" | "approved" | "neutral" | "done";

export type PillIcon =
  | "conflict"
  | "changes"
  | "human"
  | "approved"
  | "waiting"
  | "integrated"
  | "closed";

export interface Pill {
  tone: PillTone;
  icon: PillIcon;
  text: string;
  /** The same text with its people as actors, for the pill to draw them as @names. */
  segments?: Segments;
}

function verdictsOn(version: number, reviews: readonly Review[]): Review[] {
  return reviews
    .filter((r) => r.version === version && r.verdict !== "comment")
    .sort((a, b) => a.created_at - b.created_at);
}

/** The workers and humans whose latest verdict on the version is an approval, in the order they approved. */
export function approvers(
  version: number,
  reviews: readonly Review[],
): string[] {
  return approvals(version, reviews).map((r) => r.reviewer);
}

/** The approvals that stand on the version: each reviewer's latest verdict when it is an approval, in the order they approved. */
export function approvals(
  version: number,
  reviews: readonly Review[],
): Review[] {
  const last = new Map<string, Review>();
  for (const r of verdictsOn(version, reviews)) {
    last.delete(r.reviewer);
    last.set(r.reviewer, r);
  }
  return [...last.values()].filter((r) => r.verdict === "approve");
}

/** ` (kept from v3)` after a verdict a human edit kept from the version it was made from; empty for the version's own verdict. */
export function keptNote(review: Pick<Review, "kept_from">): string {
  return review.kept_from === null
    ? ""
    : ` (kept from v${String(review.kept_from)})`;
}

export interface PillContext {
  /** The main revision an integrated proposal became, when the screen knows it. */
  revision?: string | null;
  /** The review batch an integrated proposal integrated with, when the screen knows it. */
  batch?: string | null;
  /** The main revision the conflict report merged onto, which the long wording names. */
  conflictMain?: string | null;
  /** The detail header's wording; the list's otherwise. */
  long?: boolean;
}

/** `Integrated with batch rb-3 as r8`, with each part the screen knows. */
export function integratedText(
  revision: string | null,
  batch: string | null,
): string {
  const together = batch ? ` with batch ${batch}` : "";
  return revision
    ? `Integrated${together} as ${revision}`
    : `Integrated${together}`;
}

/** The review state as the summary states it: the latest verdict on the current version and whether the human review mark is met. */
export function reviewPill(
  p: ProposalSummary,
  context: PillContext = {},
): Pill {
  const { revision = null, batch = null, conflictMain = null } = context;
  const long = context.long === true;
  if (p.state === "integrated") {
    return {
      tone: "done",
      icon: "integrated",
      text: integratedText(revision, batch),
    };
  }
  if (p.state === "closed") {
    return { tone: "neutral", icon: "closed", text: "Closed" };
  }
  const back = long ? " · back with the author" : "";
  if (p.conflicted) {
    const onto = long && conflictMain ? ` with main ${conflictMain}` : "";
    return {
      tone: "danger",
      icon: "conflict",
      text: `Conflict${onto}${back}`,
    };
  }
  if (p.latest_verdict === "request_changes" || p.turn === "with_author") {
    return {
      tone: "danger",
      icon: "changes",
      text: `Changes requested${back}`,
    };
  }
  if (p.latest_verdict === "approve") {
    if (p.human_review && !p.human_approved) {
      if (long) {
        return {
          tone: "human",
          icon: "human",
          text: "Agent approved · waiting for human review",
        };
      }
      const segments = seg`Agent approved · needs ${reviewerSegment(p.human_review.reviewer, "a human")}`;
      return {
        tone: "human",
        icon: "human",
        text: segmentsText(segments),
        segments,
      };
    }
    return { tone: "approved", icon: "approved", text: "Approved" };
  }
  return { tone: "neutral", icon: "waiting", text: "Waiting for review" };
}

/** Gold is only for a wait on a person; a waiting review and an end state are quiet. */
export const PILL_CLASSES: Record<PillTone, string> = {
  danger: "bg-bad-soft text-bad",
  human: "bg-warn-soft text-warn",
  approved: "bg-ok-soft text-ok",
  neutral: "bg-fill text-text",
  done: "bg-fill text-text",
};

/**
 * The list's one state chip: the review state and whose turn it is, such as `Changes requested · waits for @web-7`. Gold is only for a proposal that waits for the signed-in human (`waitsForYou`); a review that waits for someone else is quiet, and an integrated or closed proposal shows its end state.
 */
export function listPill(
  p: ProposalSummary,
  context: { revision?: string | null; waitsForYou?: boolean } = {},
): Pill {
  const pill = reviewPill(p, { revision: context.revision ?? null });
  if (p.state !== "open") return pill;
  const head = pill.icon === "human" ? "Agent approved" : pill.text;
  if (context.waitsForYou === true) {
    return { tone: "human", icon: pill.icon, text: `${head} · waits for you` };
  }
  const tone = pill.tone === "human" ? "neutral" : pill.tone;
  const waiting = waitingOn(p);
  if (waiting === null) return { tone, icon: pill.icon, text: head };
  const who = waiting.actor === null ? "a human" : actorSegment(waiting.actor);
  const segments =
    pill.icon === "waiting"
      ? seg`${head} · ${who}`
      : seg`${head} · waits for ${who}`;
  return { tone, icon: pill.icon, text: segmentsText(segments), segments };
}

/** The list's line under a proposal's title, after its task: the version, the review claim of an open proposal and the human review mark. */
export function listMeta(p: ProposalSummary): Segments {
  const parts: Segments[] = [[versionText(p)]];
  if (p.state === "open") parts.push(claimLine(p));
  if (p.human_review) {
    parts.push(
      seg`human review by ${reviewerSegment(p.human_review.reviewer, "any human")}`,
    );
  }
  return parts.flatMap((part, index) =>
    index === 0 ? part : [" · ", ...part],
  );
}

/** Whose turn it is: an actor, or null for any human when the human-review mark names no one. */
export interface Waiting {
  actor: string | null;
}

/**
 * The author while the turn is theirs. While it is the reviewer's, the human the mark names once an agent approved without the human approval, else the review claim's holder; nobody when none of them is named.
 */
export function waitingOn(p: ProposalSummary): Waiting | null {
  if (p.turn === "with_author") return { actor: p.author };
  if (p.turn !== "with_reviewer") return null;
  if (p.latest_verdict === "approve" && p.human_review && !p.human_approved) {
    return { actor: reviewerActor(p.human_review.reviewer) };
  }
  if (p.review_claim === null) return null;
  return { actor: p.review_claim };
}

/** `v2 of 2`: the current version is always the latest. */
export function versionText(p: ProposalSummary): string {
  const n = String(p.current_version);
  return `v${n} of ${n}`;
}

export function versionRef(id: string, version: number): string {
  return `${id} v${String(version)}`;
}

const VERDICT_WORDS: Record<Verdict, string> = {
  approve: "approved",
  request_changes: "changes requested",
  reject: "rejected",
  comment: "commented",
};

/** The latest verdict on the version, for its tab, with the version a kept approval came from; comments are not verdicts. */
export function versionVerdict(
  version: number,
  reviews: readonly Review[],
): string {
  const latest = verdictsOn(version, reviews).at(-1);
  return latest
    ? `${VERDICT_WORDS[latest.verdict]}${keptNote(latest)}`
    : "no verdict yet";
}

const REVIEW_HEADS: Record<Verdict, string> = {
  approve: "Approved",
  request_changes: "Requested changes",
  reject: "Rejected",
  comment: "Comment",
};

/** `Requested changes: missing index`, as the reviews list draws a verdict. */
/** A kept approval names the version it was kept from: a human edit keeps the agent approvals of the version it was made from. */
export function reviewText(review: Review): string {
  const head =
    review.kept_from === null
      ? REVIEW_HEADS[review.verdict]
      : `${REVIEW_HEADS[review.verdict]} (kept from v${String(review.kept_from)})`;
  const body = review.body.trim();
  return body ? `${head}: ${body}` : head;
}

const RULE_WORDS: Record<ReviewScopeRule, string> = {
  new_files: "new files",
  large_delta: "a large change",
  base_conflicts: "files merged with main by hand",
  v3_or_later: "v3 or later",
};

function scopeWord(scope: "change" | "full"): string {
  return scope === "full" ? "Full review" : "Change review";
}

/**
 * The scope line of a review of a revised version, such as `Full review since v1 · suggested for new files`; null for a review of a whole first version and for a human's review. A scope the review lead chose names its reason and what fold suggested.
 */
export function reviewScopeText(review: Review): string | null {
  const scope = review.scope;
  if (scope === null || isHumanName(review.reviewer)) {
    return null;
  }
  const head = `${scopeWord(scope.scope)} since v${String(scope.since)}`;
  const rules = scope.rules.map((rule) => RULE_WORDS[rule]).join(", ");
  const suggested =
    scope.suggested === "full"
      ? `fold suggested a full review for ${rules}`
      : "fold suggested a change review";
  if (scope.reason === null) {
    return scope.suggested === "full"
      ? `${head} · suggested for ${rules}`
      : head;
  }
  return scope.suggested === scope.scope
    ? `${head} · ${scope.reason}`
    : `${head} · chosen: ${scope.reason} · ${suggested}`;
}

/**
 * The reviews list, newest first. A line comment arrives in a `comment` review of its own with no body; its thread shows it, so the list leaves that review out.
 */
export function listedReviews(reviews: readonly Review[]): Review[] {
  return reviews
    .filter((r) => r.verdict !== "comment" || r.body.trim() !== "")
    .sort((a, b) => b.created_at - a.created_at);
}

export function fileCount(v: ProposalVersion): number {
  return v.files_added + v.files_modified + v.files_deleted;
}

/** `3 files · +96 −4`; the line counts are the totals of the version the payload compared with its base. */
export function filesText(
  v: ProposalVersion,
  totals: ComparisonTotals,
): string {
  const n = fileCount(v);
  const files = `${String(n)} ${n === 1 ? "file" : "files"}`;
  return `${files} · +${String(totals.added_lines)} −${String(totals.removed_lines)}`;
}

/** `@dev, marked by @backend-lead`: who approves for the human review mark, and who set it. */
export function humanReviewFact(mark: HumanReviewMark): Segments {
  return seg`${reviewerSegment(mark.reviewer, "any human")}, marked by ${actorSegment(mark.set_by)}`;
}

/** The list's claim line under the pill. */
export function claimLine(p: ProposalSummary): Segments {
  return p.review_claim
    ? seg`claimed by ${actorSegment(p.review_claim)}`
    : ["unclaimed"];
}

export function currentVersion(p: ProposalDetail): ProposalVersion | null {
  return (
    p.versions.find((v) => v.version === p.summary.current_version) ?? null
  );
}

/** Who made a version: the author's worker, the reviewer's worker for a fix the reviewer made in its review checkout, or the human of a human edit. */
export function versionAuthor(v: ProposalVersion): string {
  return bareName(v.created_by);
}

/** Who wrote a version's evidence: a reviewer's fix carries the reviewer's list of fixes and its checks. */
export function evidenceSource(v: ProposalVersion | undefined): string {
  return v?.reviewer_fix === true ? "the reviewer" : "the author";
}

/** The evidence as lines; a line that starts with `$` is a command, the rest its output. */
export function evidenceLines(
  evidence: string,
): { text: string; command: boolean }[] {
  return evidence
    .split("\n")
    .map((text) => ({ text, command: text.startsWith("$") }));
}

/** The area of the worker that wrote the proposal; null for a worker fold does not run, or one outside every area. */
export function authorArea(
  list: Pick<WorkerList, "workers"> | undefined,
  author: string,
): string | null {
  return list?.workers.find((w) => w.name === author)?.area ?? null;
}
