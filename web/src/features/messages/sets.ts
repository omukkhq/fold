import { isApiError } from "@/api/errors";
import type { AnsweredScope, MessageView, ThreadView } from "@/api/messages";
import type { NeedsYouItem } from "@/api/workers";
import { itemsFor } from "@/app/needsYouModel";
import { isHumanName } from "@/people/names";
import { chatMessages, threadsByMessage, type AgentChat } from "./chats";

/** One question of a set as the card shows it. */
export type SetQuestion = MessageView["questions"][number];

/** An ask that waits for the signed-in human: one set of 1 to 4 questions, with the thread its answer goes to. */
export interface OpenSet {
  message: MessageView;
  thread: ThreadView;
}

/**
 * The asks that wait for the signed-in human, by message id: the question items of Needs you that are theirs.
 * The daemon decides which asks are open, so the chat rows, the Messages tab and Needs you count the same sets.
 */
export function openAskIds(
  items: readonly NeedsYouItem[],
  you: string | null,
): Set<string> {
  return new Set(
    itemsFor(items, you).flatMap((item) =>
      item.kind === "question" && item.message !== null ? [item.message] : [],
    ),
  );
}

/** How many sets wait for the signed-in human: the Messages tab and the Waits for you choice show it. */
export function waitingSets(
  items: readonly NeedsYouItem[],
  you: string | null,
): number {
  return openAskIds(items, you).size;
}

/** The open sets of the chat, oldest first: the card shows the first, and the rest wait hidden behind it. */
export function chatSets(
  chat: AgentChat,
  open: ReadonlySet<string>,
): OpenSet[] {
  const threads = threadsByMessage(chat);
  return chatMessages(chat).flatMap((message) => {
    const thread = threads.get(message.id);
    return thread !== undefined && open.has(message.id)
      ? [{ message, thread }]
      : [];
  });
}

/** The questions of an ask; a message that the daemon sends without any reads as one question of its body. */
export function setQuestions(message: MessageView): SetQuestion[] {
  return message.questions.length > 0
    ? message.questions
    : [{ text: message.body.trim(), header: null, choices: [] }];
}

/** The longest question text that the card shows as a heading; a longer one, or one of several lines, reads as body text. */
export const HEADING_QUESTION = 160;

export function headingQuestion(text: string): boolean {
  return text.length <= HEADING_QUESTION && !/[\r\n]/.test(text);
}

/** `1 of 3`. */
export function stepText(step: number, total: number): string {
  return `${String(step + 1)} of ${String(total)}`;
}

/** `1 more set waits`, `2 more sets wait`; null when no other set waits. */
export function moreSetsText(more: number): string | null {
  if (more <= 0) return null;
  return more === 1 ? "1 more set waits" : `${String(more)} more sets wait`;
}

/** The answers of the shown set before fold sends them: the question on screen and the answer of each question so far. */
export interface SetDraft {
  ask: string;
  step: number;
  answers: (string | null)[];
}

export function startDraft(ask: string, questions: number): SetDraft {
  return {
    ask,
    step: 0,
    answers: Array.from({ length: questions }, () => null),
  };
}

/** The question the card shows: a complete draft whose send failed still shows its last question. */
export function shownStep(draft: SetDraft): number {
  return Math.max(Math.min(draft.step, draft.answers.length - 1), 0);
}

/** The answer of the question on screen; the next question shows, or the draft is complete after the last. After a failed send the last question takes a new answer. */
export function answerStep(draft: SetDraft, answer: string): SetDraft {
  const text = answer.trim();
  if (text === "") return draft;
  const at = shownStep(draft);
  const answers = draft.answers.map((a, i) => (i === at ? text : a));
  return { ...draft, step: at + 1, answers };
}

/** Back to the question before the one on screen, whose answer stays until the person picks another. */
export function stepBack(draft: SetDraft): SetDraft {
  if (draft.step === 0) return draft;
  return { ...draft, step: Math.max(shownStep(draft) - 1, 0) };
}

export function draftComplete(draft: SetDraft): boolean {
  return (
    draft.step >= draft.answers.length && draft.answers.every((a) => a !== null)
  );
}

/** The answers in the order of the questions, for the one reply; empty until every question has one. */
export function draftAnswers(draft: SetDraft): string[] {
  return draftComplete(draft)
    ? draft.answers.flatMap((a) => (a === null ? [] : [a]))
    : [];
}

/** The own answer the field starts with: the earlier answer of the question when it is no choice, so Back keeps a written answer. */
export function ownStart(draft: SetDraft, question: SetQuestion): string {
  const earlier = draft.answers[draft.step] ?? null;
  if (earlier === null) return "";
  return question.choices.some((c) => c.label === earlier) ? "" : earlier;
}

/** The choice of the question on screen that a number key picks: `1` the first, up to the number of choices; null for any other key. */
export function choiceKey(key: string, choices: number): number | null {
  if (!/^[1-9]$/.test(key)) return null;
  const index = Number(key) - 1;
  return index < choices ? index : null;
}

/** The choice a key press on the card picks: its number key without a modifier, never a key typed into a field. */
export function keyChoice(
  event: {
    key: string;
    altKey: boolean;
    ctrlKey: boolean;
    metaKey: boolean;
    target: { tagName?: string; isContentEditable?: boolean } | null;
  },
  choices: number,
): number | null {
  if (event.altKey || event.ctrlKey || event.metaKey) return null;
  if (typesText(event.target)) return null;
  return choiceKey(event.key, choices);
}

/** A key typed into a field belongs to the field, never to the card's number keys. */
export function typesText(
  target: { tagName?: string; isContentEditable?: boolean } | null,
): boolean {
  if (target === null) return false;
  const tag = target.tagName?.toLowerCase() ?? "";
  return (
    tag === "input" ||
    tag === "textarea" ||
    tag === "select" ||
    target.isContentEditable === true
  );
}

/** One question of an answered set with the answer that the reply gave it. */
export interface AnsweredPair {
  question: string;
  answer: string;
}

/** The question and answer pairs of a reply that answered a set of the thread with the answer route; null for any other message, or for a set of one, whose reply is the answer alone. */
export function answeredPairs(
  reply: MessageView,
  thread: ThreadView,
): AnsweredPair[] | null {
  if (reply.answers_ask === null || reply.answers.length < 2) return null;
  const ask = thread.messages.find((m) => m.id === reply.answers_ask);
  if (ask === undefined) return null;
  const questions = setQuestions(ask);
  return reply.answers.map((answer, i) => ({
    question: questions[i]?.text ?? "",
    answer,
  }));
}

/** The agent that answered a set for the person, such as main, which the timeline names over the answers; null when the person answered it. */
export function answeredFor(reply: MessageView): string | null {
  return reply.answers_ask !== null && !isHumanName(reply.sender)
    ? reply.sender
    : null;
}

/** A set of several questions, which the timeline lists under its body; a single question is its body. */
export function listedQuestions(message: MessageView): SetQuestion[] {
  return message.questions.length > 1 ? message.questions : [];
}

/** What Mark answered closes: the set on the card alone, so another set of its thread keeps waiting; without a card, the asks of a thread that a human can close. A worker's `AskUserQuestion` ends only with an answer, so its set offers none. */
export function markScope(
  set: OpenSet | null,
  closable: ThreadView | undefined,
): AnsweredScope | null {
  if (set !== null) {
    return set.message.worker_question ? null : { ask: set.message.id };
  }
  return closable === undefined ? null : { thread: closable.id };
}

/** The set the card showed has left the open sets, as its answer makes it do. */
export function answeredAway(
  shown: string | null,
  sets: readonly OpenSet[],
): boolean {
  return shown !== null && !sets.some((s) => s.message.id === shown);
}

/** The daemon refused the answers because the set was closed meanwhile, such as by main or Mark answered. */
export function alreadyAnswered(error: unknown): boolean {
  return isApiError(error, "ask_not_open");
}

/** The line that stays once the shown set has left: the answers this screen sent, or a set that someone else answered or closed. */
export function leftText(sentHere: boolean): string {
  return sentHere ? "Answers sent." : "This set was already answered.";
}
