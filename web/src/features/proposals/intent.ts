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
  /** The version before, when this version's intent says exactly what that one's does, as a human edit that kept it; null otherwise. */
  keptFrom: number | null;
}

/** What the tick beside a part says to a screen reader: the part holds its own files of the diff. */
export const PART_COVERS = ", covers its files";

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
  const intent = versionIntent(entry.version);
  const before = entries.find(
    (item) => item.version.version === entry.version.version - 1,
  );
  const earlier = before === undefined ? null : versionIntent(before.version);
  return {
    version: entry.version.version,
    intent,
    by: entry.version.created_by,
    origin: entry.origin,
    summary: reviewerSummary(entry.reviews),
    keptFrom:
      intent !== null &&
      earlier !== null &&
      before !== undefined &&
      sameIntent(intent, earlier)
        ? before.version.version
        : null,
  };
}

/** Whether two intents say the same: an unchanged edit form keeps the edited version's intent, and a version with the same intent as the one before kept it. The daemon sends the fields in one order, so their text compares them. */
export function sameIntent(a: Intent, b: Intent): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Who the head of the Intent card names: who wrote the intent, the version it was kept from, or nobody for a version without an intent. */
export type IntentByline =
  | { kind: "written"; words: string; by: string }
  | { kind: "kept"; from: number }
  | null;

const WRITTEN: Record<HistoryEntry["origin"], string> = {
  author: "written by",
  review_fix: "written in a review fix by",
  human_edit: "written in a human edit by",
};

export function intentByline(model: IntentCardModel): IntentByline {
  if (model.intent === null) return null;
  if (model.keptFrom !== null) return { kind: "kept", from: model.keptFrom };
  return { kind: "written", words: WRITTEN[model.origin], by: model.by };
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

/** The line under the summary: that every changed file belongs to a part, or how many belong to none, which then needs a reader's attention. */
export interface Coverage {
  text: string;
  /** The changed files that no part names. */
  outside: number;
}

/** The coverage of the version's changed files, `changed` as its diff names them, by the parts of its intent. */
export function coverage(intent: Intent, changed: readonly string[]): Coverage {
  const named = new Set(intent.parts.flatMap((part) => part.files));
  const outside = changed.filter((path) => !named.has(path)).length;
  if (outside > 0) {
    return {
      text:
        outside === 1
          ? "1 changed file belongs to no part."
          : `${String(outside)} changed files belong to no part.`,
      outside,
    };
  }
  const where = intent.parts.length === 1 ? "the part below" : "a part below";
  const files = changed.length;
  const text =
    files === 0
      ? "This version changes no file."
      : files === 1
        ? `The changed file belongs to ${where}.`
        : files === 2
          ? `Both changed files belong to ${where}.`
          : `All ${String(files)} changed files belong to ${where}.`;
  return { text, outside: 0 };
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
