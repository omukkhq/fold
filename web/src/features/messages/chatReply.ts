import type { MessageView, ThreadView } from "@/api/messages";
import { threadContext, type AgentChat } from "./chats";
import { replyTarget, type ReplyTarget } from "./replyTarget";
import type { OpenSet } from "./sets";
import { threadTitle } from "./title";

/** One choice of what a reply in the chat is about: a thread by its id. */
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

/** Every thread of the chat, newest first. */
export function aboutOptions(chat: AgentChat): AboutOption[] {
  return chat.threads.map((t) => ({ value: t.id, label: threadLabel(t) }));
}

/** The thread the reply box writes in: the one the person chose, else the thread of the set on the card, else the newest thread. */
export function aboutValue(
  chat: AgentChat,
  set: OpenSet | null,
  about: string | null,
): string | null {
  if (about !== null && chat.threads.some((t) => t.id === about)) return about;
  return set?.thread.id ?? chat.threads[0]?.id ?? null;
}

/** What a reply in the chat sends to, and the thread it joins. */
export interface ChatReplyTarget extends ReplyTarget {
  thread: ThreadView;
}

/** The reply box replies in the chosen thread as that thread's own reply box would; the card answers the set's questions. A reply never answers an open set, so it never claims to; `open` holds the asks that wait. */
export function chatReplyTarget(
  chat: AgentChat,
  you: string | null,
  set: OpenSet | null,
  about: string | null,
  open: ReadonlySet<string>,
): ChatReplyTarget | null {
  const value = aboutValue(chat, set, about);
  const thread = chat.threads.find((t) => t.id === value);
  if (thread === undefined) return null;
  const target = replyTarget(thread, you);
  if (target === null) return null;
  const answers =
    target.answers !== null && open.has(target.answers.id)
      ? null
      : target.answers;
  return { ...target, answers, thread };
}

/** The reply box's note while the thread it writes in holds an open set: only the card's answers or Mark answered close a set. */
export const REPLY_HINT =
  "Your reply does not answer the open set; answer it in the card.";

/** The ask is a stored question set. The daemon also gives a question with choices and a marked ask one question, but a human's reply in their thread ends them, as it ends a worker's `AskUserQuestion` of one question; a set has neither choices nor the mark. */
export function isStoredSet(message: MessageView): boolean {
  return (
    message.questions.length > 0 &&
    message.choices.length === 0 &&
    !message.asks_human
  );
}

export function replyHint(
  thread: ThreadView | null,
  open: ReadonlySet<string>,
): string | null {
  return thread !== null &&
    thread.messages.some((m) => open.has(m.id) && isStoredSet(m))
    ? REPLY_HINT
    : null;
}
