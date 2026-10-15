import { api } from "./client";
import { unwrap } from "./result";
import { readPages, type Pages } from "./pages";
import type { components } from "./schema";

type Schemas = components["schemas"];

export type Revision = Schemas["RevisionView"];
export type TreeEntry = Schemas["TreeEntryView"];
export type FileView = Schemas["FileContentView"];
export type RepositoryState = Schemas["RepositoryView"];

const PAGE_SIZE = 200;

/** Main's head, the path of its read-only copy, and the newest revisions. */
export async function getRepository(slug: string): Promise<RepositoryState> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/repository", {
      params: { path: { slug } },
    }),
  );
}
const MAX_PAGES = 5;

/** The main revisions, newest first, in at most `MAX_PAGES` pages of `PAGE_SIZE`; `more` says that older revisions were left out. */
export async function listRevisions(slug: string): Promise<Pages<Revision>> {
  return readPages(async (before) => {
    const page = await unwrap(
      api.GET("/api/v1/projects/{slug}/revisions", {
        params: { path: { slug }, query: { before, limit: PAGE_SIZE } },
      }),
    );
    return { items: page.revisions, next: page.before };
  }, MAX_PAGES);
}

/** The entries of one directory (`""` is the root) in name order. Rejects with `revision_not_found` for a revision the project does not have. */
export async function getTree(
  slug: string,
  revision: string,
  dir: string,
): Promise<TreeEntry[]> {
  const { entries } = await unwrap(
    api.GET("/api/v1/projects/{slug}/revisions/{r}/tree", {
      params: { path: { slug, r: revision }, query: { path: dir } },
    }),
  );
  return entries;
}

/** One file. A binary or too-large file carries its size and hash and no text. */
export async function getFile(
  slug: string,
  revision: string,
  path: string,
): Promise<FileView> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/revisions/{r}/file", {
      params: { path: { slug, r: revision }, query: { path } },
    }),
  );
}
