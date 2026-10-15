import { api } from "./client";
import { unwrap } from "./result";
import { readPages, type Pages } from "./pages";
import type { components } from "./schema";

type Schemas = components["schemas"];

export type RecipientView = Schemas["RecipientView"];
export type MessageView = Schemas["MessageView"];
export type ThreadView = Schemas["ThreadView"];

/** A human reply (`reply_to`) or feedback linked to a task. The daemon picks the recipients. */
export type NewMessage = { body: string } & (
  | { reply_to: string; task?: undefined }
  | { task: string; reply_to?: undefined }
);

const PAGE_SIZE = 200;
const MAX_PAGES = 5;

/** The newest threads first, in at most `MAX_PAGES` pages of `PAGE_SIZE`; `more` says that older threads were left out. */
export async function listThreads(slug: string): Promise<Pages<ThreadView>> {
  return readPages(async (before) => {
    const page = await unwrap(
      api.GET("/api/v1/projects/{slug}/messages", {
        params: { path: { slug }, query: { before, limit: PAGE_SIZE } },
      }),
    );
    return { items: page.items, next: page.next };
  }, MAX_PAGES);
}

/**
 * Sends a human reply or feedback and answers with the stored message. The message changes no task state.
 * Rejects with the daemon's codes, such as `message_not_found` or `human_name_required`.
 */
export async function sendMessage(
  slug: string,
  input: NewMessage,
): Promise<MessageView> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/feedback", {
      params: { path: { slug } },
      body: input,
    }),
  );
}

/** How many threads in the whole project hold a question to a human that no human has answered. One request for a page of a single thread carries the project-wide count. */
export async function countAwaitingHuman(slug: string): Promise<number> {
  const page = await unwrap(
    api.GET("/api/v1/projects/{slug}/messages", {
      params: { path: { slug }, query: { limit: 1 } },
    }),
  );
  return page.total_awaiting_human;
}
