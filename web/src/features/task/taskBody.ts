import { isApiError } from "@/api/errors";

/** What the task dialog's body shows for the state of its task read. */
export type TaskBody<T> =
  | { kind: "loading" }
  | { kind: "missing" }
  | { kind: "failed" }
  | {
      kind: "loaded";
      task: T;
      /** The error of a read again that failed; the task the daemon sent last stays on screen below it. */
      failure: Error | null;
    };

/**
 * A failed read again keeps the task the reader is looking at, with its scroll position, and only a read that never
 * returned a task shows the full error state. A task the daemon reports missing is gone, not a failure, so it shows
 * the missing state even over a task read earlier.
 */
export function taskBody<T>(
  data: T | undefined,
  error: Error | null,
): TaskBody<T> {
  if (isApiError(error, "task_not_found")) {
    return { kind: "missing" };
  }
  if (data !== undefined) {
    return { kind: "loaded", task: data, failure: error };
  }
  return error === null ? { kind: "loading" } : { kind: "failed" };
}
