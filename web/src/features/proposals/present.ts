import type {
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

function humanReviewer(p: ProposalSummary): string {
  return p.human_review?.reviewer ?? "a human";
}

/**
 * The review state as the payload states it. `long` is the detail header's wording, the default the list's.
 * `conflictMain` is the main revision the conflict report merged onto, which the long wording names.
 */
export function reviewPill(
  p: ProposalSummary,
  long = false,
  conflictMain: string | null = null,
): Pill {
  if (p.state === "integrated") {
    return {
      tone: "done",
      icon: "integrated",
      text: `Integrated as ${p.integrated_revision ?? "main"}`,
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
  if (p.latest_verdict === "request_changes") {
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
          : `Agent approved · needs ${humanReviewer(p)}`,
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

export function waitingOn(p: ProposalSummary): Waiting | null {
  if (p.waiting_on === null) return null;
  return { name: displayName(p.waiting_on), human: isHuman(p.waiting_on) };
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
  const verdicts = reviews
    .filter((r) => r.version === version && r.verdict !== "comment")
    .sort((a, b) => a.created_at - b.created_at);
  const latest = verdicts.at(-1);
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

export function filesText(v: ProposalVersion): string {
  const files = `${String(v.files)} ${v.files === 1 ? "file" : "files"}`;
  return `${files} · +${String(v.added_lines)} −${String(v.removed_lines)}`;
}

export function claimText(p: ProposalSummary): string {
  return p.review_claim
    ? `review claimed by ${p.review_claim}`
    : "review not claimed";
}

export function currentVersion(p: ProposalDetail): ProposalVersion | null {
  return p.versions.find((v) => v.version === p.current_version) ?? null;
}

/** The evidence as lines; a line that starts with `$` is a command, the rest its output. */
export function evidenceLines(
  evidence: string,
): { text: string; command: boolean }[] {
  return evidence
    .split("\n")
    .map((text) => ({ text, command: text.startsWith("$") }));
}
