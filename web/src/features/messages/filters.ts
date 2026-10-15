import type { ThreadView } from "@/api/messages";
import { isHumanName } from "@/people/names";

export interface ThreadFilter {
  agent?: string;
  task?: string;
  proposal?: string;
}

export const isHuman = isHumanName;

/** The threads that match every set field, newest first. An agent matches by taking part in the thread. */
export function filterThreads(
  threads: ThreadView[],
  filter: ThreadFilter,
): ThreadView[] {
  return threads
    .filter(
      (t) =>
        (filter.agent === undefined || t.participants.includes(filter.agent)) &&
        (filter.task === undefined || t.task === filter.task) &&
        (filter.proposal === undefined || t.proposal === filter.proposal),
    )
    .sort((a, b) => b.last_at - a.last_at);
}

function sorted(values: Iterable<string>): string[] {
  return [...new Set(values)].sort((a, b) =>
    a.localeCompare(b, undefined, { numeric: true }),
  );
}

/** What each filter can pick, taken from the threads themselves. */
export function filterOptions(threads: ThreadView[]): {
  agents: string[];
  tasks: string[];
  proposals: string[];
} {
  return {
    agents: sorted(
      threads.flatMap((t) => t.participants).filter((p) => !isHuman(p)),
    ),
    tasks: sorted(threads.flatMap((t) => (t.task === null ? [] : [t.task]))),
    proposals: sorted(
      threads.flatMap((t) => (t.proposal === null ? [] : [t.proposal])),
    ),
  };
}

/** The thread with this id, or the thread that holds a message with it. */
export function findThread(
  threads: ThreadView[],
  id: string | undefined,
): ThreadView | null {
  if (id === undefined) {
    return null;
  }
  return (
    threads.find((t) => t.id === id) ??
    threads.find((t) => t.messages.some((m) => m.id === id)) ??
    null
  );
}

/** The recipients a reply in the thread reaches, for the composer's hint. */
export function replyTargets(thread: ThreadView): string[] {
  return thread.participants.filter((p) => !isHuman(p));
}

/** The filter button's text: `Agent` while it shows every thread, `Agent: frontend` once set. */
export function filterLabel(label: string, value: string | undefined): string {
  return value === undefined ? label : `${label}: ${value}`;
}
