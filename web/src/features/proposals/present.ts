import type {
  ComparisonTotals,
  ProposalDetail,
  ProposalSummary,
  ProposalVersion,
  Review,
  Verdict,
} from "@/api/proposals";

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
}

const HUMAN_PREFIX = "human:";

export function isHuman(name: string): boolean {
  return name.startsWith(HUMAN_PREFIX);
}

/** A human shows by the name they signed in with, a worker by its full name. */
export function displayName(name: string): string {
  return isHuman(name) ? name.slice(HUMAN_PREFIX.length) : name;
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
  const last = new Map<string, Review>();
  for (const r of verdictsOn(version, reviews)) {
    last.delete(r.reviewer);
    last.set(r.reviewer, r);
  }
  return [...last.values()]
    .filter((r) => r.verdict === "approve")
    .map((r) => r.reviewer);
}

export interface PillContext {
  /** The main revision an integrated proposal became, when the screen knows it. */
  revision?: string | null;
  /** The main revision the conflict report merged onto, which the long wording names. */
  conflictMain?: string | null;
  /** The detail header's wording; the list's otherwise. */
  long?: boolean;
}

/** The review state as the summary states it: the latest verdict on the current version and whether the human review mark is met. */
export function reviewPill(
  p: ProposalSummary,
  context: PillContext = {},
): Pill {
  const { revision = null, conflictMain = null } = context;
  const long = context.long === true;
  if (p.state === "integrated") {
    return {
      tone: "done",
      icon: "integrated",
      text: revision ? `Integrated as ${revision}` : "Integrated",
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
      return {
        tone: "human",
        icon: "human",
        text: long
          ? "Agent approved · waiting for human review"
          : `Agent approved · needs ${p.human_review.reviewer ?? "a human"}`,
      };
    }
    return { tone: "approved", icon: "approved", text: "Approved" };
  }
  return { tone: "neutral", icon: "waiting", text: "Waiting for review" };
}

export const PILL_CLASSES: Record<PillTone, string> = {
  danger: "bg-danger-soft text-danger",
  human: "bg-secondary-soft text-secondary-text",
  approved: "bg-primary-soft text-primary-text",
  neutral: "bg-sunken text-text-muted",
  done: "bg-ink-soft text-text",
};

export interface Waiting {
  name: string;
  human: boolean;
}

/**
 * The author while the turn is theirs. While it is the reviewer's, the human the mark names once an agent approved without the human approval, else the review claim's holder; nobody when none of them is named.
 */
export function waitingOn(p: ProposalSummary): Waiting | null {
  if (p.turn === "with_author") {
    return { name: displayName(p.author), human: isHuman(p.author) };
  }
  if (p.turn !== "with_reviewer") return null;
  if (p.latest_verdict === "approve" && p.human_review && !p.human_approved) {
    return { name: p.human_review.reviewer ?? "a human", human: true };
  }
  if (p.review_claim === null) return null;
  return { name: displayName(p.review_claim), human: isHuman(p.review_claim) };
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

/** The latest verdict on the version, for its tab; comments are not verdicts. */
export function versionVerdict(
  version: number,
  reviews: readonly Review[],
): string {
  const latest = verdictsOn(version, reviews).at(-1);
  return latest ? VERDICT_WORDS[latest.verdict] : "no verdict yet";
}

const REVIEW_HEADS: Record<Verdict, string> = {
  approve: "Approved",
  request_changes: "Requested changes",
  reject: "Rejected",
  comment: "Comment",
};

/** `Requested changes: missing index`, as the reviews list draws a verdict. */
export function reviewText(review: Review): string {
  const head = REVIEW_HEADS[review.verdict];
  const body = review.body.trim();
  return body ? `${head}: ${body}` : head;
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

export function claimText(p: ProposalSummary): string {
  return p.review_claim
    ? `review claimed by ${p.review_claim}`
    : "review not claimed";
}

export function currentVersion(p: ProposalDetail): ProposalVersion | null {
  return (
    p.versions.find((v) => v.version === p.summary.current_version) ?? null
  );
}

/** The evidence as lines; a line that starts with `$` is a command, the rest its output. */
export function evidenceLines(
  evidence: string,
): { text: string; command: boolean }[] {
  return evidence
    .split("\n")
    .map((text) => ({ text, command: text.startsWith("$") }));
}
