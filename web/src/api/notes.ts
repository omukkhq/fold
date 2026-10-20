import { api } from "./client";
import { unwrap } from "./result";
import { readPages, type Pages } from "./pages";
import type { components } from "./schema";

export type NoteView = components["schemas"]["NoteView"];

const PAGE_SIZE = 200;
const MAX_PAGES = 5;

/** The newest notes first, in at most `MAX_PAGES` pages of `PAGE_SIZE`; `more` says that older notes were left out. */
export async function listNotes(slug: string): Promise<Pages<NoteView>> {
  return readPages(async (before) => {
    const page = await unwrap(
      api.GET("/api/v1/projects/{slug}/notes", {
        params: { path: { slug }, query: { before, limit: PAGE_SIZE } },
      }),
    );
    return { items: page.items, next: page.next };
  }, MAX_PAGES);
}

/** How many notes the signed-in human has not read; null when fold does not know the human's name. One request for a page of a single note carries the count. */
export async function countUnreadNotes(slug: string): Promise<number | null> {
  const page = await unwrap(
    api.GET("/api/v1/projects/{slug}/notes", {
      params: { path: { slug }, query: { limit: 1 } },
    }),
  );
  return page.unread;
}

/**
 * Marks every note up to `through` as read by the signed-in human and answers with the human's unread count after it.
 * Rejects with the daemon's codes, such as `human_name_required`.
 */
export async function markNotesRead(
  slug: string,
  through: string,
): Promise<number> {
  const read = await unwrap(
    api.POST("/api/v1/projects/{slug}/notes/read", {
      params: { path: { slug } },
      body: { through },
    }),
  );
  return read.unread;
}
