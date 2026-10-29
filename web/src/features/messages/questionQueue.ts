import type { MessageView, ThreadView } from "@/api/messages";
import { threadCanMarkAnswered } from "./answered";
import {
  chatMessages,
  groupChats,
  threadsByMessage,
  type AgentChat,
} from "./chats";
import { isHumanName } from "@/people/names";
import { askWaits, asksYou } from "./replyTarget";

/** An ask that waits for the signed-in human, with the thread its answer goes to. */
export interface OpenQuestion {
  message: MessageView;
  thread: ThreadView;
}

/**
 * The message of a thread the daemon marks as waiting for a human that the client cannot see as an ask, such as a worker's question without choices: the newest agent message to the signed-in human with no human message after it.
 */
export function unmarkedAsk(
  thread: ThreadView,
  you: string | null,
): MessageView | null {
  if (
    !thread.awaiting_human ||
    thread.messages.some((m) => askWaits(thread, m))
  ) {
    return null;
  }
  const last = thread.messages.findLastIndex((m) => isHumanName(m.sender));
  return (
    thread.messages
      .slice(last + 1)
      .findLast((m) => m.sender !== "fold" && asksYou(m, you)) ?? null
  );
}

/**
 * The asks of the chat that wait for the signed-in human, oldest first: the order the chat asks them in.
 * A question with choices also ends when someone marks it answered, which only the thread's `awaiting_human` shows, so an ask waits only in a thread the daemon marks as waiting.
 */
export function openQuestions(
  chat: AgentChat,
  you: string | null,
): OpenQuestion[] {
  const threads = threadsByMessage(chat);
  const unmarked = new Set(
    chat.threads.flatMap((t) => unmarkedAsk(t, you)?.id ?? []),
  );
  return chatMessages(chat).flatMap((message) => {
    const thread = threads.get(message.id);
    if (thread === undefined) return [];
    const waits =
      unmarked.has(message.id) ||
      (thread.awaiting_human &&
        askWaits(thread, message) &&
        asksYou(message, you));
    return waits ? [{ message, thread }] : [];
  });
}

/** How many asks wait for the signed-in human across the threads: the sum of the chat rows' gold counts, which the Messages tab and the Waits for you filter show. */
export function waitingQuestions(
  threads: readonly ThreadView[],
  you: string | null,
): number {
  return groupChats(threads).reduce(
    (sum, chat) => sum + openQuestions(chat, you).length,
    0,
  );
}

/** The question the dock shows: the one the person stepped to, or the place it had in the queue once it left, such as after its answer. */
export interface QuestionCursor {
  id: string | null;
  index: number;
}

/** The index of the question the dock shows, or -1 for an empty queue. An answered question leaves the queue, so the next one takes its place. */
export function queueIndex(
  queue: readonly OpenQuestion[],
  cursor: QuestionCursor,
): number {
  if (queue.length === 0) return -1;
  const at = queue.findIndex((q) => q.message.id === cursor.id);
  if (at >= 0) return at;
  return Math.min(Math.max(cursor.index, 0), queue.length - 1);
}

/** The first question of the dock: the linked question, else the oldest one of the linked thread, else the oldest of the chat. */
export function startCursor(
  queue: readonly OpenQuestion[],
  anchor: string | null,
): QuestionCursor {
  const own = queue.findIndex((q) => q.message.id === anchor);
  const linked =
    own >= 0
      ? own
      : queue.findIndex((q) => q.thread.messages.some((m) => m.id === anchor));
  const index = Math.max(linked, 0);
  return { id: queue[index]?.message.id ?? null, index };
}

/** The cursor on the question at `index`. */
export function cursorAt(
  queue: readonly OpenQuestion[],
  index: number,
): QuestionCursor {
  return { id: queue[index]?.message.id ?? null, index };
}

/** `Question 2 of 3`, or `Question` while only one waits. */
export function stepText(index: number, total: number): string {
  return total <= 1
    ? "Question"
    : `Question ${String(index + 1)} of ${String(total)}`;
}

/** The thread that Mark answered closes the asks of: the current question's while one shows, so the control never closes a question the dock does not show; without one, the newest thread of the chat with an ask a human can close. Null when that thread has no such ask. */
export function markableThread(
  chat: AgentChat,
  current: OpenQuestion | null,
): ThreadView | null {
  if (current !== null) {
    return threadCanMarkAnswered(current.thread) ? current.thread : null;
  }
  return chat.threads.find(threadCanMarkAnswered) ?? null;
}

/** The question the dock showed has left the queue, as an answer makes it do. */
export function answeredAway(
  shown: string | null,
  queue: readonly OpenQuestion[],
): boolean {
  return shown !== null && !queue.some((q) => q.message.id === shown);
}

/** A step to either end of the queue turns off the control that made it, so the focus moves to the dock's heading instead of to nothing. */
export function stepEndsQueue(index: number, total: number): boolean {
  return index <= 0 || index >= total - 1;
}
