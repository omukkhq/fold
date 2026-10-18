import { api } from "./client";
import type { Comparison, FileDiff } from "./diff";
import { ApiError, isApiError } from "./errors";
import { unwrap } from "./result";
import type { components } from "./schema";

type Schemas = components["schemas"];

export type ProposalState = Schemas["ProposalStateView"];
export type ProposalTurn = Schemas["ProposalTurn"];
export type Verdict = Schemas["ReviewVerdict"];
export type HumanReviewMark = Schemas["HumanReviewMark"];
export type ProposalSummary = Schemas["ProposalSummary"];
export type ProposalVersion = Schemas["ProposalVersion"];
export type Review = Schemas["ProposalReview"];
export type ProposalTask = Schemas["ProposalTask"];
export type CommentSide = Schemas["CommentSide"];
export type ReviewComment = Schemas["ReviewComment"];
export type GateName = Schemas["GateName"];
export type GateStatus = Schemas["GateStatusKind"];
/** One gate as the daemon decided it for the signed-in human integrating the version. */
export type Gate = Schemas["ProposalGate"];
export type ConflictKind = Schemas["ConflictKind"];
export type ConflictEntry = Schemas["ConflictEntry"];
export type ConflictHunk = Schemas["ConflictHunk"];
export type ConflictFile = Schemas["ConflictFile"];
export type ConflictReport = Schemas["ConflictReportView"];
export type ProposalDetail = Schemas["ProposalDetail"];
export type ComparisonTotals = Schemas["ComparisonTotals"];
/** A line comment, or a comment on the version as a whole when the four range fields are absent. */
export type NewComment = Schemas["AddCommentBody"];
export type NewReview = Schemas["ReviewBody"];
export type SubmittedReview = Schemas["SubmittedReview"];
export type IntegrationResult = Schemas["IntegrationResult"];
/** One run of the project's checks by fold, with each command's result. */
export type CheckRun = Schemas["CheckRun"];
export type CheckResult = Schemas["CheckResult"];
export type CheckOutcome = Schemas["CheckOutcome"];
export type CheckRunState = Schemas["CheckRunState"];
export type CheckRunReason = Schemas["CheckRunReason"];
/** One file of a proposal version, as a human edit starts from it. */
export type VersionFile = Schemas["FileContentView"];
/** A human's new content for one text file of the version the human read. */
export type NewEdit = Schemas["EditFileBody"];

function base(slug: string, id: string) {
  return { path: { slug, id } };
}

export async function listProposals(slug: string): Promise<ProposalSummary[]> {
  const { proposals } = await unwrap(
    api.GET("/api/v1/projects/{slug}/proposals", {
      params: { path: { slug } },
    }),
  );
  return proposals;
}

/** The proposal with the gate report for the signed-in human and the current version; null for a proposal the project does not have. */
export async function getProposal(
  slug: string,
  id: string,
): Promise<ProposalDetail | null> {
  try {
    return await unwrap(
      api.GET("/api/v1/projects/{slug}/proposals/{id}", {
        params: base(slug, id),
      }),
    );
  } catch (error) {
    if (isApiError(error, "proposal_not_found")) return null;
    throw error;
  }
}

/** Which change of a proposal to read: `version` against its base, or against the earlier version `since`. Both are numbers, so the answer never changes for the same range. */
export interface ChangeRange {
  version: number;
  since: number | null;
}

function rangeQuery(range: ChangeRange) {
  return range.since === null
    ? { version: range.version }
    : { version: range.version, since: range.since };
}

/** The changed files of a version against its base, or of what changed after `since`. */
export async function getProposalChanges(
  slug: string,
  id: string,
  range: ChangeRange,
): Promise<Comparison> {
  const view = await unwrap(
    api.GET("/api/v1/projects/{slug}/proposals/{id}/diff", {
      params: { ...base(slug, id), query: rangeQuery(range) },
    }),
  );
  if (view.view !== "files") {
    throw new ApiError("internal_error", "the daemon sent one file");
  }
  return view;
}

/** The hunks of one changed file of the range. On a change since an earlier version the new side's line numbers are the version's own. */
export async function getProposalFileDiff(
  slug: string,
  id: string,
  range: ChangeRange,
  path: string,
): Promise<FileDiff> {
  const view = await unwrap(
    api.GET("/api/v1/projects/{slug}/proposals/{id}/diff", {
      params: { ...base(slug, id), query: { ...rangeQuery(range), path } },
    }),
  );
  if (view.view !== "file") {
    throw new ApiError("internal_error", "the daemon sent a file list");
  }
  return view;
}

/** Every comment of every version in number order; the screens place them by the version each was written on. */
export async function listComments(
  slug: string,
  id: string,
): Promise<ReviewComment[]> {
  const { comments } = await unwrap(
    api.GET("/api/v1/projects/{slug}/proposals/{id}/comments", {
      params: base(slug, id),
    }),
  );
  return comments;
}

/** The daemon stores the comment as a `comment` review of its own on `comment.version`, the version the human read; another current version rejects with `stale_version`. */
export async function addComment(
  slug: string,
  id: string,
  comment: NewComment,
): Promise<ReviewComment> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/proposals/{id}/comments", {
      params: base(slug, id),
      body: comment,
    }),
  );
}

/** A verdict on `review.version`, the version the human read; rejects with the daemon's codes, such as `stale_version` or `empty_review`. */
export async function submitReview(
  slug: string,
  id: string,
  review: NewReview,
): Promise<SubmittedReview> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/proposals/{id}/reviews", {
      params: base(slug, id),
      body: review,
    }),
  );
}

/**
 * The human approves the version on screen and integrates it in one action. `expectedVersion` is that version: when another one is current the daemon refuses with `stale_version`.
 * A conflicting merge rejects with `integration_conflicted`, and the report is on the proposal.
 */
export async function integrateProposal(
  slug: string,
  id: string,
  expectedVersion: number,
): Promise<IntegrationResult> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/proposals/{id}/integrate", {
      params: base(slug, id),
      body: { expected_version: expectedVersion },
    }),
  );
}

/** One check run with the stored end of each command's output; a proposal lists its runs without the logs. */
export async function getCheckRun(slug: string, id: string): Promise<CheckRun> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/check-runs/{id}", {
      params: base(slug, id),
    }),
  );
}

/** One file of `version`, the content the edit box starts from. */
export async function getProposalFile(
  slug: string,
  id: string,
  version: number,
  path: string,
): Promise<VersionFile> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/proposals/{id}/file", {
      params: { ...base(slug, id), query: { version, path } },
    }),
  );
}

/**
 * A human edit of one file of `edit.version`, the version the human read, in a human review. The daemon answers with the new version; another current version rejects with `stale_version`, and a busy task worker with `worker_busy`.
 */
export async function editProposalFile(
  slug: string,
  id: string,
  edit: NewEdit,
): Promise<ProposalVersion> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/proposals/{id}/edits", {
      params: base(slug, id),
      body: edit,
    }),
  );
}

/** Sends `version`, a human edit, for review again: the approvals it kept stop counting. */
export async function sendForReviewAgain(
  slug: string,
  id: string,
  version: number,
): Promise<ProposalSummary> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/proposals/{id}/review-again", {
      params: base(slug, id),
      body: { version },
    }),
  );
}
