import { ApiError } from "./errors";
import { fromFixture } from "./fixture";
import type { components } from "./schema";

export type ChangeKind = components["schemas"]["ChangeKind"];

/** A changed file of a comparison. A binary file is compared by hash only; a hash is null on the side where the file does not exist. */
export type ComparedFile = components["schemas"]["ChangedFile"] & {
  old_hash: string | null;
  new_hash: string | null;
};

export type StateKind =
  | "revision"
  | "changeset"
  | "proposal_version"
  | "checkpoint"
  | "workspace";

/** One side of a comparison as the daemon resolved the `from` or `to` reference. */
export interface ComparedState {
  kind: StateKind;
  /** The public id: `r7`, `C-18`, `P-3 v2`, `ck-31` or `ws-7`. */
  id: string;
  /** The base revision of a workspace; null for every other kind. */
  base: string | null;
  /** The task a workspace or a proposal belongs to. */
  task: string | null;
  /** The worker of a workspace. */
  worker: string | null;
}

export interface Comparison {
  from: ComparedState;
  to: ComparedState;
  /** When the daemon read the working files, in UTC milliseconds; null when neither side is a workspace. */
  read_at: number | null;
  files: ComparedFile[];
}

export type HunkLineKind = "context" | "add" | "remove";

export interface HunkLine {
  kind: HunkLineKind;
  text: string;
}

/** A unified hunk; `section` is the text the daemon shows after the line ranges, such as the enclosing function. */
export interface Hunk {
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  section: string;
  lines: HunkLine[];
}

export interface FileDiff {
  path: string;
  binary: boolean;
  hunks: Hunk[];
}

/** The fixture keys each comparison by `from..to`, or by `to` alone for `to` against its recorded base. */
export interface ComparisonFixture {
  comparison: Comparison;
  diffs: FileDiff[];
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/diff")
  : undefined;

export function pairKey(from: string | undefined, to: string): string {
  return from === undefined ? to : `${from}..${to}`;
}

/**
 * Without `from` the daemon compares `to` with the base it recorded for it, such as a workspace's base revision or a changeset's parent.
 * Null when there is nothing to compare; a production build holds no fixture and answers null, and the screen shows its empty state.
 */
export async function getComparison(
  slug: string,
  from: string | undefined,
  to: string,
): Promise<Comparison | null> {
  const entry = await fromFixture(
    fixture,
    (m) => m.comparisons[pairKey(from, to)] ?? null,
    null,
  );
  return entry ? entry.comparison : null;
}

/** Rejects with `path_not_in_comparison` for a path the comparison does not list. */
export async function getFileDiff(
  slug: string,
  from: string | undefined,
  to: string,
  path: string,
): Promise<FileDiff> {
  const entry = await fromFixture(
    fixture,
    (m) => m.comparisons[pairKey(from, to)] ?? null,
    null,
  );
  const diff = entry?.diffs.find((d) => d.path === path);
  if (!diff) {
    throw new ApiError(
      "path_not_in_comparison",
      `${path} is not in the comparison`,
    );
  }
  return diff;
}
