import type { ThreadView } from "@/api/messages";
import { atName, bareName, isHumanName, isYou } from "@/people/names";
import { isAsk } from "./replyTarget";
import { threadTitle } from "./title";

export interface ThreadFilter {
  agent?: string;
  task?: string;
  proposal?: string;
  /** Only the threads with an ask that waits for you, as the daemon marks them. */
  waiting?: boolean;
  /** Only the threads that hold a question or another ask to a human. */
  questions?: boolean;
  /** Only the threads the signed-in human started. */
  mine?: boolean;
}

/** The thread holds a question with choices or a message that asks a human. */
export function hasAsk(thread: ThreadView): boolean {
  return thread.messages.some(isAsk);
}

/** The signed-in human wrote the thread's first message. */
export function startedBy(thread: ThreadView, you: string | null): boolean {
  const first = thread.messages[0];
  return first !== undefined && isYou(first.sender, you);
}

/** The threads that match every set field, newest first. An agent matches by taking part in the thread. */
export function filterThreads(
  threads: ThreadView[],
  filter: ThreadFilter,
  you: string | null = null,
): ThreadView[] {
  return threads
    .filter(
      (t) =>
        (filter.agent === undefined || t.participants.includes(filter.agent)) &&
        (filter.task === undefined || t.task === filter.task) &&
        (filter.proposal === undefined || t.proposal === filter.proposal) &&
        (filter.waiting !== true || t.awaiting_human) &&
        (filter.questions !== true || hasAsk(t)) &&
        (filter.mine !== true || startedBy(t, you)),
    )
    .sort((a, b) => b.last_at - a.last_at);
}

function sorted(values: Iterable<string>): string[] {
  return [...new Set(values)].sort((a, b) =>
    a.localeCompare(b, undefined, { numeric: true }),
  );
}

/** How many listed threads the daemon marks as waiting for you: the threads the Messages tab badge counts. */
export function waitingCount(threads: readonly ThreadView[]): number {
  return threads.filter((t) => t.awaiting_human).length;
}

/** The waiting filter's text, with the number of threads it shows. */
export function waitingLabel(count: number): string {
  return count === 0 ? "Waits for you" : `Waits for you · ${String(count)}`;
}

/** What each filter can pick, taken from the threads themselves. */
export function filterOptions(threads: ThreadView[]): {
  agents: string[];
  tasks: string[];
  proposals: string[];
} {
  return {
    agents: sorted(
      threads.flatMap((t) => t.participants).filter((p) => !isHumanName(p)),
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

/** The recipients a reply in the thread reaches, for the composer's hint. fold takes part in a thread only with its own notes and never reads a reply. */
export function replyTargets(thread: ThreadView): string[] {
  return thread.participants.filter((p) => !isHumanName(p) && p !== "fold");
}

/** The reply box's placeholder: `Reply to @frontend/ui, @backend…`, or `Reply…` when the thread has no agent. */
export function replyPlaceholder(thread: ThreadView): string {
  const targets = replyTargets(thread);
  return targets.length > 0
    ? `Reply to ${targets.map(atName).join(", ")}…`
    : "Reply…";
}

/** The filter button's text: `Agent` while it shows every thread, `Agent: frontend` once set. */
export function filterLabel(label: string, value: string | undefined): string {
  return value === undefined ? label : `${label}: ${value}`;
}

/** The threads whose title, messages, people, task, proposal or id hold every word of `query`, without case. */
export function searchThreads(
  threads: ThreadView[],
  query: string,
): ThreadView[] {
  const words = query
    .toLowerCase()
    .split(/\s+/)
    .filter((w) => w !== "");
  if (words.length === 0) return threads;
  return threads.filter((t) => {
    const text = [
      threadTitle(t),
      t.id,
      t.task ?? "",
      t.proposal ?? "",
      ...t.participants.map((p) => `@${bareName(p)}`),
      ...t.messages.map((m) => m.body),
    ]
      .join("\n")
      .toLowerCase();
    return words.every((word) => text.includes(word));
  });
}

/** The chip row above the thread list shows all threads or one kind of them. */
export type ShowFilter = "all" | "waiting" | "questions" | "mine";

export function showFilter(filter: ThreadFilter): ShowFilter {
  if (filter.waiting === true) return "waiting";
  if (filter.questions === true) return "questions";
  if (filter.mine === true) return "mine";
  return "all";
}

/** The filter with only `show` among the chip filters, the other fields kept. */
export function withShow<T extends ThreadFilter>(
  filter: T,
  show: ShowFilter,
): T {
  return {
    ...filter,
    waiting: show === "waiting" ? true : undefined,
    questions: show === "questions" ? true : undefined,
    mine: show === "mine" ? true : undefined,
  };
}

export interface ShowChip {
  show: ShowFilter;
  label: string;
}

/** The chips: All, the waiting count the Messages tab badge also counts, Questions, and Mine while a human is signed in. */
export function showChips(
  threads: readonly ThreadView[],
  you: string | null,
): ShowChip[] {
  return [
    { show: "all", label: "All" },
    { show: "waiting", label: waitingLabel(waitingCount(threads)) },
    { show: "questions", label: "Questions" },
    ...(you === null ? [] : [{ show: "mine" as const, label: "Mine" }]),
  ];
}
