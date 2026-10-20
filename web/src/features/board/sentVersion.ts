import type { AttentionItem, BoardProposal } from "@/api/board";
import type { LiveWorkspace } from "@/live/envelope";
import { hasKind } from "./attention";

/** What a sent version waits for, the first that applies in this order: a conflict, failed integration checks, changes requested, a human's approval, the integration of an approved version, a review. */
export type SentWait =
  | "conflict"
  | "checks_failed"
  | "changes_requested"
  | "human_review"
  | "integration"
  | "review";

export interface SentVersion {
  proposal: string;
  version: number;
  waits: SentWait;
}

/** The proposal fields the decision reads; the board card and the task view both carry them. */
export type SentProposal = Pick<
  BoardProposal,
  "id" | "state" | "version" | "checks_failed"
>;

/** Items are typed loosely because the daemon can be newer than this build and send a kind it does not know. */
type Items = readonly AttentionItem[] | readonly { kind: string }[];

const WAITS: Record<SentWait, string> = {
  conflict: "conflicts with main",
  checks_failed: "integration checks failed",
  changes_requested: "changes requested",
  human_review: "waits for human review",
  integration: "approved, waits for integration",
  review: "waits for review",
};

function waitOf(proposal: SentProposal, attention: Items): SentWait {
  if (proposal.state === "conflict") {
    return "conflict";
  }
  if (proposal.checks_failed) {
    return "checks_failed";
  }
  if (proposal.state === "changes_requested") {
    return "changes_requested";
  }
  if (hasKind(attention, "human_review")) {
    return "human_review";
  }
  return proposal.state === "approved" ? "integration" : "review";
}

/**
 * The holder's open proposal version, while the workspace files are still that version: the daemon compares their
 * content, never their times. A live frame of the same proposal is newer than the payload, so its answer wins; without
 * one the payload's `edits_after_version` decides. Null while anything differs, when nothing was compared, and
 * without an open proposal.
 */
export function sentVersion(
  proposal: SentProposal | null,
  editsAfterVersion: boolean | null,
  live: Pick<LiveWorkspace, "proposal" | "edits_after_version"> | null,
  attention: Items,
): SentVersion | null {
  if (proposal === null || proposal.state === "integrated") {
    return null;
  }
  const fromLive =
    live !== null && live.proposal === proposal.id
      ? live.edits_after_version
      : null;
  if ((fromLive ?? editsAfterVersion) !== false) {
    return null;
  }
  return {
    proposal: proposal.id,
    version: proposal.version,
    waits: waitOf(proposal, attention),
  };
}

/** The waits that put the next move with the author, so the daemon's stall rule counts again while the files still hold the sent version. */
const WITH_AUTHOR: ReadonlySet<SentWait> = new Set<SentWait>([
  "conflict",
  "checks_failed",
  "changes_requested",
]);

/**
 * Whether the line shows the stall the daemon reports: always without a sent version, and with one only while the
 * proposal is back with its author. A version that waits on others never stalls in the daemon, so a stalled item
 * beside one belongs to an older state.
 */
export function showsStall(
  sent: SentVersion | null,
  attention: Items,
): boolean {
  return (
    hasKind(attention, "stalled") &&
    (sent === null || WITH_AUTHOR.has(sent.waits))
  );
}

/** `sent P-1 v2`. */
export function sentText(sent: SentVersion): string {
  return `sent ${sent.proposal} v${String(sent.version)}`;
}

/** `waits for review`, `changes requested`. */
export function waitText(wait: SentWait): string {
  return WAITS[wait];
}
