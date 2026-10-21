import type { QueryKey } from "@tanstack/react-query";
import type { AnsweredScope } from "@/api/messages";
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

/** The note under the control once the daemon answered: none when it closed an ask, otherwise why nothing changed. */
export function answeredNote(
  closed: readonly string[] | undefined,
): string | null {
  if (closed === undefined || closed.length > 0) {
    return null;
  }
  return "Nothing was marked: a worker's question ends only with your reply.";
}
