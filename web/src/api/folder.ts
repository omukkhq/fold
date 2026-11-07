import { api } from "./client";
import { unwrap } from "./result";
import type { components } from "./schema";

export type ResetResult = components["schemas"]["ResetResult"];

/** Puts the whole project folder back to main after a safety stash, and drops the human change that waits for the main agent with its merge workspace. Rejects with `invalid_request` and a `reason` such as `head_moved` or `skipped_tracked`. */
export async function resetFolder(slug: string): Promise<ResetResult> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/folder/reset", {
      params: { path: { slug } },
    }),
  );
}

/** Puts the named paths of the project folder back to main after a safety stash. Rejects with `invalid_request` and the reason `stash_conflicted` while an unstash conflict waits. */
export async function restorePaths(
  slug: string,
  paths: readonly string[],
): Promise<ResetResult> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/folder/restore", {
      params: { path: { slug } },
      body: { paths: [...paths] },
    }),
  );
}
