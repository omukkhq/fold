import type { ProposalVersion } from "@/api/proposals";
import type { HistoryEntry, HistoryReview } from "./history";

/** The longest intent the daemon takes, in characters. */
export const MAX_INTENT_CHARS = 2000;

/** What the top of the Overview shows: the current version's intent beside the reviewer's summary of it. */
export interface IntentCardModel {
  version: number;
  /** null for a version without an intent, such as one made before fold asked for one. */
  intent: string | null;
  /** Who made the version: the author, the reviewer of a fix or the human of an edit, who may have kept the intent of the version the edit started from. */
  by: string;
  origin: HistoryEntry["origin"];
  /** The newest review with a verdict, else the newest comment; null before any review of the version. */
  summary: HistoryReview | null;
}

/** The text a version without an intent shows in its place. */
export const NO_INTENT = "This version has no intent.";

/** The newest review with a verdict, which says what the reviewer found; a comment only when no verdict exists. */
export function reviewerSummary(
  reviews: readonly HistoryReview[],
): HistoryReview | null {
  return (
    reviews.find((item) => item.review.verdict !== "comment") ??
    reviews[0] ??
    null
  );
}

/** The card of the current version, from the history entries, newest first; null when the proposal has no version. */
export function intentCard(
  entries: readonly HistoryEntry[],
): IntentCardModel | null {
  const entry = entries.find((item) => item.current) ?? entries[0];
  if (entry === undefined) return null;
  return {
    version: entry.version.version,
    intent: versionIntent(entry.version),
    by: entry.version.created_by,
    origin: entry.origin,
    summary: reviewerSummary(entry.reviews),
  };
}

/** Whether a History card shows its version's intent: not on the version whose intent the Intent card above already shows. */
export function intentInHistory(
  entry: Pick<HistoryEntry, "version">,
  card: Pick<IntentCardModel, "version"> | null,
): boolean {
  return card === null || card.version !== entry.version.version;
}

/** A version's intent with its outer space trimmed; null when it has none. */
export function versionIntent(
  v: Pick<ProposalVersion, "intent">,
): string | null {
  const text = v.intent?.trim() ?? "";
  return text === "" ? null : text;
}
