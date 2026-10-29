import type { QueryKey } from "@tanstack/react-query";
import type { AttentionItem } from "@/api/board";
import type { AnsweredScope, ThreadView } from "@/api/messages";
import { keys } from "@/api/keys";

/** The reads that show an ask waiting: the threads and the Messages count, the board's question mark, Needs you and, for a task, its task read. */
export function answeredKeys(slug: string, scope: AnsweredScope): QueryKey[] {
  return [
    keys.messages(slug),
    keys.board(slug),
    keys.needsYou(slug),
    ...(scope.task === undefined ? [] : [keys.task(slug, scope.task)]),
  ];
}

/** A task offers Mark answered while its question mark holds an ask a human can close; a worker's `AskUserQuestion` ends only with a reply, so it offers none. */
export function taskCanMarkAnswered(
  attention: readonly AttentionItem[],
): boolean {
  return attention.some((item) => item.kind === "question" && item.closable);
}

/** A thread offers Mark answered while it waits on an ask a human can close; a thread with only a worker's `AskUserQuestion` offers the reply box alone. */
export function threadCanMarkAnswered(
  thread: Pick<ThreadView, "awaiting_human" | "closable">,
): boolean {
  return thread.awaiting_human && thread.closable;
}

/** The note under the control once the daemon answered: none when it closed an ask; otherwise the ask was answered before the click, such as by another human. */
export function answeredNote(
  closed: readonly string[] | undefined,
): string | null {
  if (closed === undefined || closed.length > 0) {
    return null;
  }
  return "It was already answered.";
}

/** Mark answered forgets the note and the error of its last call once it acts on another thread than the one it last offered, so they never show beside a question they are not about. A control that only went away keeps them. */
export function markResetDue(
  last: string | null,
  next: string | null,
): boolean {
  return last !== null && next !== null && last !== next;
}
