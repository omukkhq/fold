import type { MessageView, ThreadView } from "@/api/messages";
import { isHumanName, isYou } from "@/people/names";
import { isQuestion, questionState } from "./question";

/** A message that waits for a human's answer: a question with choices, or one its agent marked as asking a human. */
export function isAsk(message: MessageView): boolean {
  return isQuestion(message) || message.asks_human;
}

/** The message names the signed-in human as a recipient; with no one signed in, any human recipient counts. */
export function asksYou(message: MessageView, you: string | null): boolean {
  return message.recipients.some((r) =>
    you === null ? isHumanName(r.name) : isYou(r.name, you),
  );
}

/**
 * The ask still waits: no human wrote after it. An ask without choices also ends when a human marks the thread's asks answered, which the thread shows as no longer waiting.
 */
export function askWaits(thread: ThreadView, message: MessageView): boolean {
  if (!isAsk(message) || questionState(thread, message).answer !== null) {
    return false;
  }
  return isQuestion(message) || thread.awaiting_human;
}

/** A free-text ask that waits for the signed-in human: its head shows that it waits for their answer. */
export function freeAskWaitsForYou(
  thread: ThreadView,
  message: MessageView,
  you: string | null,
): boolean {
  return (
    !isQuestion(message) && askWaits(thread, message) && asksYou(message, you)
  );
}

/** What the reply box sends to: the message it replies to and, when the text answers a question, the agent who asked it. */
export interface ReplyTarget {
  replyTo: string;
  answers: MessageView | null;
}

/**
 * The reply box answers the question a human picked with "write your own answer" while it waits, else the newest waiting ask to the signed-in human when that ask has no choices; otherwise it replies to the newest message.
 */
export function replyTarget(
  thread: ThreadView,
  you: string | null,
  picked: string | null,
): ReplyTarget | null {
  const last = thread.messages.at(-1);
  if (!last) return null;
  const chosen = thread.messages.find((m) => m.id === picked);
  if (chosen && askWaits(thread, chosen)) {
    return { replyTo: chosen.id, answers: chosen };
  }
  const newest = thread.messages.findLast(
    (m) => askWaits(thread, m) && asksYou(m, you),
  );
  if (newest && !isQuestion(newest)) {
    return { replyTo: newest.id, answers: newest };
  }
  return { replyTo: last.id, answers: null };
}
