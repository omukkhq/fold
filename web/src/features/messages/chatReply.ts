import type { ThreadView } from "@/api/messages";
import { threadContext, threadsByMessage, type AgentChat } from "./chats";
import type { OpenQuestion } from "./questionQueue";
import { askWaits, replyTarget, type ReplyTarget } from "./replyTarget";
import { threadTitle } from "./title";

/** The value of the "about" choice that answers the dock's question. */
export const ABOUT_QUESTION = "question";

/** One choice of what a reply in the chat is about: the dock's question, or a thread by its id. */
export interface AboutOption {
  value: string;
  label: string;
}

/** `T-12 · Which chart library?`, or the title alone for a thread about nothing. */
export function threadLabel(thread: ThreadView): string {
  const context = threadContext(thread);
  const title = threadTitle(thread);
  return context === null ? title : `${context.id} · ${title}`;
}

/** The answer to the dock's question while one waits, then every thread of the chat, newest first. */
export function aboutOptions(
  chat: AgentChat,
  question: OpenQuestion | null,
  step: string,
): AboutOption[] {
  return [
    ...(question === null
      ? []
      : [{ value: ABOUT_QUESTION, label: `Answer to ${step.toLowerCase()}` }]),
    ...chat.threads.map((t) => ({ value: t.id, label: threadLabel(t) })),
  ];
}

/** The choice the reply box starts with and falls back to: the dock's question while one waits, else the newest thread. */
export function aboutValue(
  chat: AgentChat,
  question: OpenQuestion | null,
  about: string | null,
): string | null {
  if (about === ABOUT_QUESTION && question !== null) return ABOUT_QUESTION;
  if (about !== null && chat.threads.some((t) => t.id === about)) return about;
  return question === null ? (chat.threads[0]?.id ?? null) : ABOUT_QUESTION;
}

/** What a reply in the chat sends to, and the thread it joins. */
export interface ChatReplyTarget extends ReplyTarget {
  thread: ThreadView;
}

/**
 * The reply box answers the question a human picked with "write your own answer" while it waits, then the dock's question while the box is about it; otherwise it replies in the chosen thread as that thread's own reply box would.
 */
export function chatReplyTarget(
  chat: AgentChat,
  you: string | null,
  question: OpenQuestion | null,
  about: string | null,
  picked: string | null,
): ChatReplyTarget | null {
  const own = picked === null ? undefined : threadsByMessage(chat).get(picked);
  const ask = own?.messages.find((m) => m.id === picked);
  if (own !== undefined && ask !== undefined && askWaits(own, ask)) {
    return { replyTo: ask.id, answers: ask, thread: own };
  }
  const value = aboutValue(chat, question, about);
  if (value === ABOUT_QUESTION && question !== null) {
    return {
      replyTo: question.message.id,
      answers: question.message,
      thread: question.thread,
    };
  }
  const thread = chat.threads.find((t) => t.id === value);
  if (thread === undefined) return null;
  const target = replyTarget(thread, you, null);
  return target === null ? null : { ...target, thread };
}
