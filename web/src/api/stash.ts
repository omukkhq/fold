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

/** Drops the entry at `position`, 0 for `stash@{0}`, for good, only while that place still holds the entry `number` the human saw. Rejects with `invalid_request` and the reason `stash_changed` when another entry is there, `stash_conflicted` while its unstash conflict waits in the folder, or `stash_entry_missing` past the end of the stack. */
export async function dropStashEntry(
  slug: string,
  position: number,
  number: number,
): Promise<StashDropResult> {
  return unwrap(
    api.DELETE("/api/v1/projects/{slug}/stash/{entry}", {
      params: { path: { slug, entry: String(position) }, query: { number } },
    }),
  );
}

/** What Clear all names: the count of entries the human saw, and the `number` of the newest, null for an empty stash. */
export interface StashSeen {
  entries: number;
  newest: number | null;
}

/** Drops every entry of the stash for good, only while it is the stash the human saw. Rejects with `invalid_request` and the reason `stash_changed` when its length or newest entry differs, or `stash_conflicted` while the newest entry's unstash conflict waits in the folder. */
export async function clearStash(
  slug: string,
  seen: StashSeen,
): Promise<StashDropResult> {
  return unwrap(
    api.DELETE("/api/v1/projects/{slug}/stash", {
      params: {
        path: { slug },
        query:
          seen.newest === null
            ? { entries: seen.entries }
            : { entries: seen.entries, newest: seen.newest },
      },
    }),
  );
}
