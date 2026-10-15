import type { ComparedFile, FileDiff, FileWarning } from "./diff";
import { ApiError } from "./errors";
import { fromFixture } from "./fixture";

/**
 * The OpenAPI schema has no proposal routes, so these types are the payload the proposal screens read; each is declared here once and gives way to the generated type when the routes are in the schema.
 * Names starting `human:` are humans; every other name is a worker.
 */
export type ProposalState = "open" | "integrated" | "closed";

export type Verdict = "comment" | "request_changes" | "approve" | "reject";

/** Who acts next on an open proposal, as the daemon states it. */
export type ProposalWith = "reviewer" | "author";

export interface HumanReviewMark {
  /** The human who must approve; null when any human may. */
  reviewer: string | null;
  /** The agent or human that set the mark. */
  marked_by: string;
  reason: string;
}

export interface ProposalSummary {
  /** @example P-4 */
  id: string;
  title: string;
  /** @example T-3 */
  task: string;
  /** The worker that opened the proposal. */
  author: string;
  state: ProposalState;
  /** The latest version; versions count from 1 and never change. */
  current_version: number;
  /** Null unless the proposal is open. */
  with: ProposalWith | null;
  /** The name the next step waits on; null unless the proposal is open. */
  waiting_on: string | null;
  /** The latest verdict on the current version; comments are not verdicts. */
  latest_verdict: Exclude<Verdict, "comment"> | null;
  human_review: HumanReviewMark | null;
  /** A human approval on the current version satisfies the mark. */
  human_approved: boolean;
  /** The worker holding the review claim. */
  review_claim: string | null;
  conflicted: boolean;
  /** The main revision an integrated proposal became, such as `r8`. */
  integrated_revision: string | null;
  /** UTC milliseconds. */
  updated_at: number;
}

export interface ProposalVersion {
  version: number;
  /** The revision the version was made from, such as `r7`. */
  base: string;
  /** The hash of the version's resulting tree. */
  tree: string;
  /** The head changeset of the version, such as `C-18`. */
  changeset: string;
  /** What the author attached, as plain text; null when nothing. */
  evidence: string | null;
  files: number;
  added_lines: number;
  removed_lines: number;
  /** UTC milliseconds. */
  created_at: number;
}

export interface Review {
  version: number;
  reviewer: string;
  verdict: Verdict;
  body: string;
  /** UTC milliseconds. */
  created_at: number;
}

export type CommentSide = "old" | "new";

export interface ReviewComment {
  /** @example rc-5 */
  id: string;
  /** The version the comment was written on. */
  version: number;
  author: string;
  /** Null for a comment on the version as a whole. */
  path: string | null;
  side: CommentSide | null;
  start_line: number | null;
  end_line: number | null;
  reply_to: string | null;
  body: string;
  /** UTC milliseconds. */
  created_at: number;
}

export type GateState = "met" | "unmet" | "waiting_human";

/** One integration gate with its state as the daemon decided it. */
export type Gate = { state: GateState } & (
  | {
      kind: "approval";
      version: number;
      /** Who approved the current version, in order. */
      approvers: string[];
      latest_verdict: Exclude<Verdict, "comment"> | null;
    }
  | ({ kind: "human_review" } & HumanReviewMark)
  | {
      kind: "integrator";
      /** The agents that carry the `integrator` tag. */
      agents: string[];
    }
  | {
      kind: "prerequisites";
      /** The prerequisites that are not Done. */
      unfinished: string[];
      total: number;
    }
  | {
      kind: "merge";
      /** The main revision the version was merged onto. */
      main: string;
      conflicting_files: number;
      /** UTC milliseconds; null before the first check. */
      checked_at: number | null;
    }
);

export type GateKind = Gate["kind"];

export type ConflictKind =
  | "both_modified_text"
  | "both_modified_binary"
  | "modify_delete"
  | "add_add"
  | "type_change";

export type EntryKind = "file" | "executable" | "symlink" | "directory";

export interface ConflictEntry {
  kind: EntryKind;
  hash: string;
}

/** The lines each side holds where both changed the same lines. */
export interface ConflictHunk {
  base: string[];
  main: string[];
  version: string[];
}

export interface ConflictFile {
  path: string;
  kind: ConflictKind;
  /** Null where the side has nothing at the path. */
  base: ConflictEntry | null;
  main: ConflictEntry | null;
  version: ConflictEntry | null;
  /** Only a text conflict has hunks. */
  hunks: ConflictHunk[];
}

export interface ConflictReport {
  version: number;
  /** The revisions and trees the merge read. */
  base: string;
  main: string;
  trees: { base: string; main: string; version: string };
  files: ConflictFile[];
}

export interface IntegrationResult {
  /** The new main revision, such as `r9`. */
  revision: string;
  version: number;
  actor: string;
  /** UTC milliseconds. */
  at: number;
}

/** What the daemon lets the signed-in human do now; the web shows these and never decides a gate. */
export interface HumanActions {
  approve: boolean;
  request_changes: boolean;
  integrate: boolean;
  comment: boolean;
}

export interface ProposalDetail extends ProposalSummary {
  /** The current main revision. */
  main: string;
  close_reason: string | null;
  versions: ProposalVersion[];
  reviews: Review[];
  comments: ReviewComment[];
  gates: Gate[];
  conflict: ConflictReport | null;
  integration: IntegrationResult | null;
  human_actions: HumanActions;
}

export interface NewReview {
  /** The version the verdict belongs to. */
  version: number;
  verdict: "approve" | "request_changes";
  body: string;
}

/** A line comment; the daemon stores it in a `comment` review of its own on that version. */
export interface NewComment {
  version: number;
  path: string;
  side: CommentSide;
  start_line: number;
  end_line: number;
  body: string;
}

export interface IntegrateRequest {
  /** The version the human approves and integrates in one action. */
  version: number;
}

/** A proposal version compared with its base or with another version. The diff route takes no proposal version, so the proposal screens read this shape; `from` and `to` are the ids the screen named, such as `P-3 v2`. */
export interface VersionComparison {
  from: string;
  to: string;
  files: ComparedFile[];
  warnings: FileWarning[];
}

export interface VersionComparisonFixture {
  comparison: VersionComparison;
  diffs: FileDiff[];
}

/** A version against its base is keyed by the version alone, two versions by `from..to`. */
export function pairKey(from: string | undefined, to: string): string {
  return from === undefined ? to : `${from}..${to}`;
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/proposals")
  : undefined;

/** The daemon's answer to a route it does not serve. */
function unserved(): ApiError {
  return new ApiError("not_found", "the daemon serves no proposal route");
}

export async function listProposals(slug: string): Promise<ProposalSummary[]> {
  return fromFixture(fixture, (m) => m.list(slug), []);
}

/** Null for a proposal the project does not have. */
export async function getProposal(
  slug: string,
  id: string,
): Promise<ProposalDetail | null> {
  return fromFixture(fixture, (m) => m.detail(slug, id), null);
}

/** Null when there is nothing to compare, such as a version the project does not have. */
export async function getVersionComparison(
  slug: string,
  from: string | undefined,
  to: string,
): Promise<VersionComparison | null> {
  const entry = await fromFixture(
    fixture,
    (m) => m.versionComparison(slug, pairKey(from, to)),
    null,
  );
  return entry ? entry.comparison : null;
}

/** Rejects with `path_not_in_comparison` for a path the comparison does not list. */
export async function getVersionFileDiff(
  slug: string,
  from: string | undefined,
  to: string,
  path: string,
): Promise<FileDiff> {
  const entry = await fromFixture(
    fixture,
    (m) => m.versionComparison(slug, pairKey(from, to)),
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

/** Resolves with the proposal as the daemon holds it after the write; rejects with the daemon's codes, such as `version_superseded`. */
export async function submitReview(
  slug: string,
  id: string,
  review: NewReview,
): Promise<ProposalDetail> {
  const answer = await fromFixture(
    fixture,
    (m) => m.review(slug, id, review),
    null,
  );
  if (!answer) throw unserved();
  return answer;
}

export async function addComment(
  slug: string,
  id: string,
  comment: NewComment,
): Promise<ProposalDetail> {
  const answer = await fromFixture(
    fixture,
    (m) => m.comment(slug, id, comment),
    null,
  );
  if (!answer) throw unserved();
  return answer;
}

/** The human approves the version and integrates it; on a conflict the daemon answers with the conflicted proposal, not an error. */
export async function integrateProposal(
  slug: string,
  id: string,
  request: IntegrateRequest,
): Promise<ProposalDetail> {
  const answer = await fromFixture(
    fixture,
    (m) => m.integrate(slug, id, request),
    null,
  );
  if (!answer) throw unserved();
  return answer;
}
