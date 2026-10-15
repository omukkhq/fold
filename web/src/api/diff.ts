import { api } from "./client";
import { ApiError } from "./errors";
import { unwrap } from "./result";
import type { components } from "./schema";
import { getWorkspace } from "./workspaces";

type Schemas = components["schemas"];

export type ChangeKind = Schemas["FileChangeKind"];
/** A changed file of a comparison. A binary file is compared by hash only; a hash is null on the side where the file does not exist. */
export type ComparedFile = Schemas["ComparedFile"];
export type StateKind = Schemas["StateKindView"];
/** One side of a comparison as the daemon resolved the `from` or `to` reference. */
export type ComparedState = Schemas["ComparedState"];
export type Comparison = Schemas["ComparisonView"];
export type FileDiff = Schemas["FileDiffView"];
export type Hunk = Schemas["HunkView"];
export type HunkLine = Schemas["DiffLineView"];
export type HunkLineKind = Schemas["DiffLineKind"];
/** A working file the daemon could not read; it keeps its base entry, so it never shows as deleted. */
export type FileWarning = Schemas["FileWarning"];

const WORKSPACE_REF = /^ws-\d+$/;

/** A link that names only `to` compares a workspace with its base revision. Any other reference has no base a route reports, so it has no default. */
async function resolveFrom(
  slug: string,
  from: string | undefined,
  to: string,
): Promise<string | null> {
  if (from !== undefined) {
    return from;
  }
  if (!WORKSPACE_REF.test(to)) {
    return null;
  }
  return (await getWorkspace(slug, to)).base;
}

/**
 * The changed files of `from` against `to`. Without `from` a workspace is compared with its base revision; any other `to` without `from` has nothing to compare and answers null.
 */
export async function getComparison(
  slug: string,
  from: string | undefined,
  to: string,
): Promise<Comparison | null> {
  const resolved = await resolveFrom(slug, from, to);
  if (resolved === null) {
    return null;
  }
  const view = await unwrap(
    api.GET("/api/v1/projects/{slug}/diff", {
      params: { path: { slug }, query: { from: resolved, to } },
    }),
  );
  if (view.view !== "files") {
    throw new ApiError("internal_error", "the daemon sent one file");
  }
  return view;
}

/** The hunks of one changed file. Rejects with `path_not_in_comparison` for a path the comparison does not list. */
export async function getFileDiff(
  slug: string,
  from: string | undefined,
  to: string,
  path: string,
): Promise<FileDiff> {
  const resolved = await resolveFrom(slug, from, to);
  if (resolved === null) {
    throw new ApiError(
      "invalid_state_ref",
      `${to} needs a state to compare with`,
    );
  }
  const view = await unwrap(
    api.GET("/api/v1/projects/{slug}/diff", {
      params: { path: { slug }, query: { from: resolved, to, path } },
    }),
  );
  if (view.view !== "file") {
    throw new ApiError("internal_error", "the daemon sent a file list");
  }
  return view;
}
