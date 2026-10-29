import type { MessageView, ThreadView } from "@/api/messages";
import { isHumanName, isYou } from "@/people/names";
import { isQuestion, questionState } from "./question";

/** A message that waits for a human's answer: a question set, a question with choices, or one its agent marked as asking a human. */
export function isAsk(message: MessageView): boolean {
  return (
    message.questions.length > 0 || isQuestion(message) || message.asks_human
  );
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

/** What the reply box sends to: the message it replies to and, when the text answers a question, the agent who asked it. */
export interface ReplyTarget {
  replyTo: string;
  answers: MessageView | null;
}

/** A free ask the reply box answers: a marked ask or a worker's question with one question and no choices. A set of several, or a question with choices, takes its answers on the set's card. */
function freeAsk(message: MessageView): boolean {
  return (
    (message.asks_human || message.worker_question) &&
    !isQuestion(message) &&
    message.questions.length <= 1
  );
}

/**
 * The reply box answers the newest waiting ask to the signed-in human when it is a free ask; otherwise it replies to the newest message.
 */
export function replyTarget(
  thread: ThreadView,
  you: string | null,
): ReplyTarget | null {
  const last = thread.messages.at(-1);
  if (!last) return null;
  const newest = thread.messages.findLast(
    (m) => askWaits(thread, m) && asksYou(m, you),
  );
  if (newest && freeAsk(newest)) {
    return { replyTo: newest.id, answers: newest };
  }
  return { replyTo: last.id, answers: null };
}
