import { api } from "./client";
import type { Comparison, FileDiff } from "./diff";
import { ApiError } from "./errors";
import { unwrap } from "./result";
import type { components } from "./schema";

type Schemas = components["schemas"];

export type SharedFile = Schemas["SharedFileView"];

/** The project folder against the revision it holds: the changed files with line counts. */
export async function getFolderComparison(slug: string): Promise<Comparison> {
  const view = await unwrap(
    api.GET("/api/v1/projects/{slug}/folder/diff", {
      params: { path: { slug }, query: {} },
    }),
  );
  if (view.view !== "files") {
    throw new ApiError("internal_error", "the daemon sent one file");
  }
  return view;
}

/** The hunks of one changed file of the project folder. Rejects with `path_not_in_comparison` for a file that holds the revision's version. */
export async function getFolderFileDiff(
  slug: string,
  path: string,
): Promise<FileDiff> {
  const view = await unwrap(
    api.GET("/api/v1/projects/{slug}/folder/diff", {
      params: { path: { slug }, query: { path } },
    }),
  );
  if (view.view !== "file") {
    throw new ApiError("internal_error", "the daemon sent a file list");
  }
  return view;
}

/** The changed shared files that wait in Needs you, with their size and change time now. */
export async function getChangedSharedFiles(
  slug: string,
): Promise<SharedFile[]> {
  const view = await unwrap(
    api.GET("/api/v1/projects/{slug}/shared/changes", {
      params: { path: { slug } },
    }),
  );
  return view.files;
}
