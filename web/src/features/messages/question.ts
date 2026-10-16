import type { MessageChoice, MessageView, ThreadView } from "@/api/messages";
import { isHumanName } from "@/people/names";

/** A message that offers choices is a question an agent asks the humans. */
export function isQuestion(message: MessageView): boolean {
  return message.choices.length > 0;
}

export interface QuestionState {
  /** The first human message after the question: the answer, a named option or free text. */
  answer: MessageView | null;
  /** The option the answer names, compared without case and surrounding space, as the daemon compares it. */
  chosen: string | null;
}

/** The label of the option `body` names, compared without case and surrounding space, as the daemon compares it. */
export function chosenOption(
  choices: readonly MessageChoice[],
  body: string,
): string | null {
  const reply = body.trim().toLowerCase();
  return (
    choices.find((choice) => choice.label.toLowerCase() === reply)?.label ??
    null
  );
}

export function questionState(
  thread: ThreadView,
  question: MessageView,
): QuestionState {
  const at = thread.messages.findIndex((m) => m.id === question.id);
  const answer =
    thread.messages.slice(at + 1).find((m) => isHumanName(m.sender)) ?? null;
  return {
    answer,
    chosen:
      answer === null ? null : chosenOption(question.choices, answer.body),
  };
}

/** The messages of the thread that answer a question by naming one of its options. */
export function chosenAnswers(thread: ThreadView): Set<string> {
  const ids = new Set<string>();
  for (const message of thread.messages) {
    if (!isQuestion(message)) continue;
    const state = questionState(thread, message);
    if (state.answer !== null && state.chosen !== null) {
      ids.add(state.answer.id);
    }
  }
  return ids;
}

/** An answer sent from this screen stays pending until the thread holds a human answer: the daemon has then stored it. */
export function answerPending(
  sending: boolean,
  sent: boolean,
  state: QuestionState,
): boolean {
  return sending || (sent && state.answer === null);
}
