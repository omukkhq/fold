import { api } from "./client";
import { unwrap } from "./result";
import { readPages, type Pages } from "./pages";
import type { components } from "./schema";

type Schemas = components["schemas"];

export type RecipientView = Schemas["RecipientView"];
export type MessageView = Schemas["MessageView"];
export type MessageChoice = Schemas["MessageChoice"];
export type ThreadView = Schemas["ThreadView"];
export type MessageLines = Schemas["MessageLines"];

/** A human reply (`reply_to`) or feedback linked to a task. The daemon picks the recipients. */
export type NewMessage = { body: string } & (
  | { reply_to: string; task?: undefined }
  | { task: string; reply_to?: undefined }
);

const PAGE_SIZE = 200;
const MAX_PAGES = 5;

/** The threads that concern the signed-in human, newest first, in at most `MAX_PAGES` pages of `PAGE_SIZE`; `more` says that older threads were left out. The daemon filters them; agent-to-agent threads stay on the task screen and in Activity. */
export async function listThreads(slug: string): Promise<Pages<ThreadView>> {
  return readPages(async (before) => {
    const page = await unwrap(
      api.GET("/api/v1/projects/{slug}/messages", {
        params: {
          path: { slug },
          query: { mine: true, before, limit: PAGE_SIZE },
        },
      }),
    );
    return { items: page.items, next: page.next };
  }, MAX_PAGES);
}

/** The threads with a message linked to the proposal, newest first, in at most `MAX_PAGES` pages of `PAGE_SIZE`; `more` says that older threads were left out. */
export async function listProposalThreads(
  slug: string,
  proposal: string,
): Promise<Pages<ThreadView>> {
  return readPages(async (before) => {
    const page = await unwrap(
      api.GET("/api/v1/projects/{slug}/messages", {
        params: {
          path: { slug },
          query: { proposal, before, limit: PAGE_SIZE },
        },
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

/** What a human marks answered: the open asks of the thread that holds a message, or every open ask linked to a task. */
export type AnsweredScope =
  | { thread: string; task?: undefined }
  | { task: string; thread?: undefined };

/**
 * Marks the open asks of a thread or a task answered without a reply, and answers with the ids it closed, none when nothing waited.
 * A worker's `AskUserQuestion` stays open, because its worker waits for a reply in the thread.
 */
export async function markAnswered(
  slug: string,
  scope: AnsweredScope,
): Promise<string[]> {
  const result = await unwrap(
    api.POST("/api/v1/projects/{slug}/asks/answered", {
      params: { path: { slug } },
      body: scope,
    }),
  );
  return result.answered;
}

/** How many threads hold an ask that waits for the signed-in human: the threads the Messages screen marks. One request for a page of a single thread carries the count. */
export async function countAwaitingHuman(slug: string): Promise<number> {
  const page = await unwrap(
    api.GET("/api/v1/projects/{slug}/messages", {
      params: { path: { slug }, query: { mine: true, limit: 1 } },
    }),
  );
  return page.total_awaiting_human;
}
