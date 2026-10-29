import type { ComparedFile } from "@/api/diff";
import type {
  Intent,
  IntentContracts,
  IntentPart,
  ProposalVersion,
} from "@/api/proposals";
import { lineCounts } from "@/features/diff/present";
import { filesWord } from "./folderTree";
import type { HistoryEntry, HistoryReview } from "./history";

/** The longest summary the daemon takes, in characters. */
export const MAX_SUMMARY_CHARS = 1200;
/** The most parts an intent has. */
export const MAX_PARTS = 12;
/** The longest part name, in characters. */
export const MAX_PART_NAME_CHARS = 80;
/** The longest text of what a part does, in characters. */
export const MAX_PART_CHARS = 800;
/** The longest entry of a contract kind, a decision or what the change does not change, in characters. */
export const MAX_ENTRY_CHARS = 300;
/** The most entries of one list. */
export const MAX_ENTRIES = 20;

/** The kinds of contract in the order a reader sees them, with their labels. */
export const CONTRACT_KINDS: readonly {
  kind: keyof IntentContracts;
  label: string;
}[] = [
  { kind: "routes", label: "Routes" },
  { kind: "database", label: "Database" },
  { kind: "rights", label: "Rights" },
  { kind: "config", label: "Config" },
  { kind: "errors", label: "Errors" },
  { kind: "events", label: "Events" },
  { kind: "commands", label: "Commands" },
];

/** What the top of the Overview shows: the current version's intent, and beside it the reviewer's summary of it. */
export interface IntentCardModel {
  version: number;
  /** null for a version without an intent, such as one made before fold asked for one. */
  intent: Intent | null;
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

/** The reviewer who checked the version against its intent: the author of the newest verdict; null while only comments or nothing stand. */
export function checkedBy(model: IntentCardModel): string | null {
  const summary = model.summary;
  return summary === null || summary.review.verdict === "comment"
    ? null
    : summary.review.reviewer;
}

/** Whether a History card shows its version's intent: not on the version whose intent the Intent card above already shows. */
export function intentInHistory(
  entry: Pick<HistoryEntry, "version">,
  card: Pick<IntentCardModel, "version"> | null,
): boolean {
  return card === null || card.version !== entry.version.version;
}

/** A version's intent; null when it has none or its summary is blank. */
export function versionIntent(
  v: Pick<ProposalVersion, "intent">,
): Intent | null {
  const intent = v.intent;
  return intent === null || intent.summary.trim() === "" ? null : intent;
}

/** One kind of contract that the intent names, with its label. */
export interface ContractLine {
  kind: keyof IntentContracts;
  label: string;
  entries: readonly string[];
}

/** The sections an intent shows besides its summary and parts: only the contract kinds, decisions and not-changed entries it has, so an empty section never shows. */
export interface IntentSections {
  contracts: ContractLine[];
  decisions: readonly string[];
  notChanged: readonly string[];
}

export function intentSections(intent: Intent): IntentSections {
  return {
    contracts: CONTRACT_KINDS.flatMap(({ kind, label }) => {
      const entries = intent.contracts[kind];
      return entries.length === 0 ? [] : [{ kind, label, entries }];
    }),
    decisions: intent.decisions,
    notChanged: intent.not_changed,
  };
}

/** The changed files the parts name, which the daemon holds to be exactly the version's changed files. */
export function intentFileCount(intent: Intent): number {
  return intent.parts.reduce((sum, part) => sum + part.files.length, 0);
}

/** The line under the summary that says every changed file belongs to a part. */
export function coverageLine(intent: Intent): string {
  const files = intentFileCount(intent);
  const where = intent.parts.length === 1 ? "the part below" : "a part below";
  if (files === 0) return "This version changes no file.";
  if (files === 1) return `The changed file belongs to ${where}.`;
  if (files === 2) return `Both changed files belong to ${where}.`;
  return `All ${String(files)} changed files belong to ${where}.`;
}

/** The part that `path` belongs to; null when no part names it. */
export function partOf(intent: Intent | null, path: string): IntentPart | null {
  return intent?.parts.find((part) => part.files.includes(path)) ?? null;
}

/** How big a part is: its file count and the lines its files add and remove in `files`, such as `2 files · +275 −12`. */
export function partSize(
  part: IntentPart,
  files: readonly Pick<
    ComparedFile,
    "path" | "added_lines" | "removed_lines"
  >[],
): string {
  const byPath = new Map(files.map((file) => [file.path, file]));
  const sum = part.files.reduce(
    (total, path) => {
      const file = byPath.get(path);
      return {
        added_lines: total.added_lines + (file?.added_lines ?? 0),
        removed_lines: total.removed_lines + (file?.removed_lines ?? 0),
      };
    },
    { added_lines: 0, removed_lines: 0 },
  );
  const counts = lineCounts(sum);
  const lines = [counts.added, counts.removed]
    .filter((text) => text !== null)
    .join(" ");
  const size = filesWord(part.files.length);
  return lines === "" ? size : `${size} · ${lines}`;
}
