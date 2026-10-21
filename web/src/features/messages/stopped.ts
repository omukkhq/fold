import type { ThreadView } from "@/api/messages";
import type { WorkerProcess } from "@/api/workers";
import { atName, isHumanName } from "@/people/names";

/** The mark of a stopped task worker in a reader choice. */
export const STOPPED_MARK = "stopped";

/** The note under a message box that reaches a stopped task worker: the daemon starts it again for a human's message. */
export const STOPPED_NOTE = "stopped · starts again for your message";

/** The agents whose task worker is stopped or failed, which a message from a human starts again. */
export function stoppedTaskWorkers(
  workers:
    | readonly Pick<WorkerProcess, "name" | "role" | "state">[]
    | undefined,
): ReadonlySet<string> {
  return new Set(
    (workers ?? [])
      .filter(
        (worker) =>
          worker.role === "task_worker" &&
          (worker.state === "stopped" || worker.state === "failed"),
      )
      .map((worker) => worker.name),
  );
}

/** Whether `name`, an agent or one of its workers such as `web-worker/ui`, belongs to a stopped task worker. */
export function isStopped(name: string, stopped: ReadonlySet<string>): boolean {
  return stopped.has(name.split("/")[0] ?? name);
}

/** The agents a human's reply in the thread reaches: the reply answers the newest message, so it goes to that message's agent sender, or, when a human or fold sent it, to its agent recipients. */
export function replyAgents(thread: ThreadView): string[] {
  const last = thread.messages.at(-1);
  if (last === undefined) {
    return [];
  }
  if (last.sender !== "fold" && !isHumanName(last.sender)) {
    return [last.sender];
  }
  return last.recipients
    .map((recipient) => recipient.name)
    .filter((name) => !isHumanName(name));
}

/** The note on the stopped task workers that a message box reaches: `@web-worker: stopped · starts again for your message`; null when none is stopped. */
export function stoppedReplyNote(
  targets: readonly string[],
  stopped: ReadonlySet<string>,
): string | null {
  const names = targets.filter((name) => isStopped(name, stopped));
  return names.length === 0
    ? null
    : `${names.map(atName).join(", ")}: ${STOPPED_NOTE}`;
}
