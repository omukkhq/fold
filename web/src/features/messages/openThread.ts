import { isApiError } from "@/api/errors";
import type { ThreadView } from "@/api/messages";
import {
  chatByKey,
  chatHolding,
  newestMessage,
  threadIn,
  type AgentChat,
} from "./chats";
import { findThread } from "./filters";
import { openQuestions } from "./questionQueue";

/** The chat pane's content: a chat of the listing, or a thread read on its own because the listing did not reach it. */
export type OpenChat =
  | { kind: "listed"; chat: AgentChat | null }
  | { kind: "unlisted"; id: string };

/** The visible chats: every chat with a thread that the filters and the search keep, with all of its threads. */
export function visibleChats(
  chats: readonly AgentChat[],
  visible: readonly ThreadView[],
): AgentChat[] {
  const ids = new Set(visible.map((t) => t.id));
  return chats.filter((c) => c.threads.some((t) => ids.has(t.id)));
}

/**
 * A linked thread or message opens the chat that holds it; a chat key opens that chat; with neither, the newest visible chat opens.
 * A requested thread that no listed thread holds is read directly: opening another chat instead would show another conversation under the requested address.
 * A chat that the filters hide falls back to the first visible chat, as the filters ask.
 */
export function openChat(
  all: ThreadView[],
  chats: readonly AgentChat[],
  visible: readonly AgentChat[],
  requested: { chat?: string; thread?: string },
): OpenChat {
  const { chat, thread } = requested;
  if (thread !== undefined && findThread(all, thread) === null) {
    return { kind: "unlisted", id: thread };
  }
  const wanted =
    thread === undefined ? chatByKey(chats, chat) : chatHolding(chats, thread);
  const shown = visible.find((c) => c.key === wanted?.key);
  return { kind: "listed", chat: shown ?? visible[0] ?? null };
}

/**
 * The message the chat opens at. A link to a message opens at it; a link to a thread opens at its oldest question to the signed-in human, else at its newest message; otherwise the chat opens at its newest message.
 * A thread's id is also the id of its first message, so such a link reads as a link to the thread.
 */
export function chatAnchor(
  chat: AgentChat,
  link: string | undefined,
  you: string | null,
): string | null {
  const thread = threadIn(chat, link);
  if (thread === null || link === undefined) {
    return newestMessage(chat)?.id ?? null;
  }
  if (thread.id !== link) return link;
  const own: AgentChat = { ...chat, threads: [thread] };
  return (
    openQuestions(own, you)[0]?.message.id ?? thread.messages.at(-1)?.id ?? null
  );
}

/** A malformed id names no message either, so it reads as missing rather than as a daemon failure. */
export function isMissingThread(error: unknown): boolean {
  return (
    isApiError(error, "message_not_found") || isApiError(error, "invalid_id")
  );
}
