import { api } from "./client";
import { unwrap } from "./result";
import type { components } from "./schema";

export type StashEntry = components["schemas"]["StashEntryView"];
export type StashList = components["schemas"]["StashList"];
export type StashDropResult = components["schemas"]["StashDropResult"];

/** The stash of the project folder, newest entry first. Rejects with `project_not_found` or `project_unavailable`. */
export async function listStash(slug: string): Promise<StashList> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/stash", { params: { path: { slug } } }),
  );
}

/** Drops the entry at `position`, 0 for `stash@{0}`, for good. Rejects with `invalid_request` and the reason `stash_conflicted` while its unstash conflict waits in the folder, or `stash_entry_missing` when the stack changed meanwhile. */
export async function dropStashEntry(
  slug: string,
  position: number,
): Promise<StashDropResult> {
  return unwrap(
    api.DELETE("/api/v1/projects/{slug}/stash/{entry}", {
      params: { path: { slug, entry: String(position) } },
    }),
  );
}

/** Drops every entry of the stash for good. Rejects with `invalid_request` and the reason `stash_conflicted` while the newest entry's unstash conflict waits in the folder. */
export async function clearStash(slug: string): Promise<StashDropResult> {
  return unwrap(
    api.DELETE("/api/v1/projects/{slug}/stash", {
      params: { path: { slug } },
    }),
  );
}
