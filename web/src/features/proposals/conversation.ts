import type { Hunk } from "@/api/diff";
import type { MessageView, ThreadView } from "@/api/messages";
import type {
  ProposalDetail,
  ProposalVersion,
  ReviewComment,
} from "@/api/proposals";
import type { DiffSelection } from "@/features/diff/selection";
import { isHumanName } from "@/people/names";
import { buildThreads, linesText, type CommentThread } from "./threadsModel";

/** What a thread of the proposal is: a human's question to the author, an agent's question to the humans, a review comment, or a message between agents. */
export type ConversationKind = "to_author" | "to_you" | "review" | "agent";

/** Where a thread stands: an ask that waits for a human, a question that waits for the author, a review comment on the current version of an open proposal, or nothing open. */
export type ConversationState =
  | "needs_answer"
  | "waits_author"
  | "open"
  | "closed";

export type ConversationSource =
  | { type: "message"; thread: ThreadView }
  | { type: "comments"; thread: CommentThread };

export interface ConversationThread {
  key: string;
  kind: ConversationKind;
  state: ConversationState;
  /** The state as the thread's pill says it, such as `Answered` or `Outdated`. */
  stateText: string;
  /** Who a question that waits for the author went to. */
  waitsFor: string[];
  /** The version the thread is about: its lines' version, else the version that was current when it started. */
  version: number | null;
  lines: DiffSelection | null;
  /** UTC milliseconds of the newest entry. */
  at: number;
  source: ConversationSource;
}

function isAgent(sender: string): boolean {
  return sender !== "fold" && !isHumanName(sender);
}

function asksHumans(message: MessageView): boolean {
  return (
    message.asks_human ||
    message.choices.length > 0 ||
    message.recipients.some((r) => isHumanName(r.name) || r.name === "humans")
  );
}

/** The version that was current at `at`: the newest version made by then, else the first. */
export function versionAt(
  versions: readonly ProposalVersion[],
  at: number,
): number | null {
  const sorted = [...versions].sort((a, b) => a.version - b.version);
  const made = sorted.filter((v) => v.created_at <= at).at(-1) ?? sorted[0];
  return made?.version ?? null;
}

function messageThread(
  p: ProposalDetail,
  thread: ThreadView,
): ConversationThread | null {
  const first = thread.messages[0];
  if (!first) return null;
  const lines = thread.messages.find((m) => m.lines !== null)?.lines ?? null;
  const base = {
    key: thread.id,
    version: lines?.version ?? versionAt(p.versions, first.sent_at),
    lines:
      lines === null
        ? null
        : {
            path: lines.path,
            side: lines.side,
            start_line: lines.start_line,
            end_line: lines.end_line,
          },
    at: thread.last_at,
    source: { type: "message" as const, thread },
    waitsFor: [] as string[],
  };
  const kind: ConversationKind = isHumanName(first.sender)
    ? "to_author"
    : thread.awaiting_human || thread.messages.some(asksHumans)
      ? "to_you"
      : "agent";
  if (thread.awaiting_human) {
    return {
      ...base,
      kind,
      state: "needs_answer",
      stateText: "Needs your answer",
    };
  }
  if (kind === "to_author") {
    // The newest human message is the open question until an agent writes after it; fold's own notes answer nothing.
    let asked = first;
    let answered = false;
    for (const m of thread.messages) {
      if (isHumanName(m.sender)) {
        asked = m;
        answered = false;
      } else if (isAgent(m.sender)) {
        answered = true;
      }
    }
    return answered
      ? { ...base, kind, state: "closed", stateText: "Answered" }
      : {
          ...base,
          kind,
          state: "waits_author",
          stateText: "Waits for the author",
          waitsFor: asked.recipients.map((r) => r.name),
        };
  }
  return {
    ...base,
    kind,
    state: "closed",
    stateText: kind === "to_you" ? "Answered" : "For information",
  };
}

function commentThread(
  p: ProposalDetail,
  thread: CommentThread,
): ConversationThread {
  const open = p.summary.state === "open";
  const state: ConversationState = open && !thread.outdated ? "open" : "closed";
  const stateText =
    state === "open" ? "Open" : open ? "Earlier version" : "Closed";
  return {
    key: thread.id,
    kind: "review",
    state,
    stateText,
    waitsFor: [],
    version: thread.version,
    lines: thread.range,
    at: Math.max(...thread.comments.map((c) => c.comment.created_at)),
    source: { type: "comments", thread },
  };
}

const STATE_ORDER: Record<ConversationState, number> = {
  needs_answer: 0,
  waits_author: 1,
  open: 2,
  closed: 3,
};

/** Every thread of the proposal: the message threads linked to it and the review comment threads of all its versions. What waits for someone comes first, then each group by its newest entry. */
export function conversationThreads(
  p: ProposalDetail,
  messages: readonly ThreadView[],
  comments: readonly ReviewComment[],
): ConversationThread[] {
  const fromMessages = messages
    .map((thread) => messageThread(p, thread))
    .filter((t): t is ConversationThread => t !== null);
  const fromComments = buildThreads(comments, p.summary.current_version).map(
    (thread) => commentThread(p, thread),
  );
  return [...fromMessages, ...fromComments].sort(
    (a, b) => STATE_ORDER[a.state] - STATE_ORDER[b.state] || b.at - a.at,
  );
}

export type StateFilter = "all" | ConversationState;

export interface ConversationFilter {
  state: StateFilter;
  /** Null for every kind. */
  kind: ConversationKind | null;
  /** Null for every version. */
  version: number | null;
}

export const NO_FILTER: ConversationFilter = {
  state: "all",
  kind: null,
  version: null,
};

function passes(
  thread: ConversationThread,
  filter: ConversationFilter,
): boolean {
  return (
    (filter.state === "all" || thread.state === filter.state) &&
    (filter.kind === null || thread.kind === filter.kind) &&
    (filter.version === null || thread.version === filter.version)
  );
}

export function filterConversation(
  threads: readonly ConversationThread[],
  filter: ConversationFilter,
): ConversationThread[] {
  return threads.filter((t) => passes(t, filter));
}

export interface FilterOption<T> {
  value: T;
  label: string;
  /** The threads the option shows with the other filters as they are. */
  count: number;
  selected: boolean;
}

const STATE_LABELS: [StateFilter, string][] = [
  ["all", "All"],
  ["needs_answer", "Needs your answer"],
  ["waits_author", "Waits for the author"],
  ["open", "Open"],
  ["closed", "Closed"],
];

const KIND_LABELS: [ConversationKind, string][] = [
  ["to_author", "Questions to the author"],
  ["to_you", "Questions to you"],
  ["review", "Review comments"],
  ["agent", "Agent messages"],
];

export function stateOptions(
  threads: readonly ConversationThread[],
  filter: ConversationFilter,
): FilterOption<StateFilter>[] {
  return STATE_LABELS.map(([value, label]) => ({
    value,
    label,
    count: filterConversation(threads, { ...filter, state: value }).length,
    selected: filter.state === value,
  }));
}

/** Every kind as an option, after the option for any of them. */
export function kindOptions(
  threads: readonly ConversationThread[],
  filter: ConversationFilter,
): FilterOption<ConversationKind | null>[] {
  return [
    {
      value: null,
      label: "Any kind",
      count: filterConversation(threads, { ...filter, kind: null }).length,
      selected: filter.kind === null,
    },
    ...KIND_LABELS.map(([value, label]) => ({
      value,
      label,
      count: filterConversation(threads, { ...filter, kind: value }).length,
      selected: filter.kind === value,
    })),
  ];
}

/** Every version a thread is about, newest first, after the option for all of them. */
export function versionOptions(
  threads: readonly ConversationThread[],
  filter: ConversationFilter,
  current: number,
): FilterOption<number | null>[] {
  const versions = [
    ...new Set(
      threads.map((t) => t.version).filter((v): v is number => v !== null),
    ),
  ].sort((a, b) => b - a);
  return [
    {
      value: null,
      label: "All versions",
      count: filterConversation(threads, { ...filter, version: null }).length,
      selected: filter.version === null,
    },
    ...versions.map((v) => ({
      value: v,
      label: v === current ? `v${String(v)} (current)` : `v${String(v)}`,
      count: filterConversation(threads, { ...filter, version: v }).length,
      selected: filter.version === v,
    })),
  ];
}

export const KIND_TEXT: Record<ConversationKind, string> = {
  to_author: "Question to the author",
  to_you: "Question to you",
  review: "Review comment",
  agent: "Agent message",
};

/** Where a thread is: `web/src/a.ts · lines 2–4`, or `whole proposal`. */
export function whereText(thread: ConversationThread): string {
  return thread.lines === null
    ? "whole proposal"
    : `${thread.lines.path} · ${linesText(thread.lines)}`;
}

/** The version note: `v4`, or `on v2 · outdated` for lines of an earlier version that a later version changed. */
export function versionText(
  thread: ConversationThread,
  current: number,
  changed: boolean,
): string | null {
  if (thread.version === null) return null;
  const v = `v${String(thread.version)}`;
  if (thread.version >= current) return v;
  return changed ? `on ${v} · outdated` : `on ${v}`;
}

/** The change since the thread's version that tells whether a later version changed its lines: only lines of the version's own side have one. */
export function laterChange(
  thread: ConversationThread,
  current: number,
): { since: number; path: string } | null {
  const { lines, version } = thread;
  if (lines === null || version === null || version >= current) return null;
  return lines.side === "new" ? { since: version, path: lines.path } : null;
}

/** Whether the change since a version removed or rewrote any of `range`'s lines of that version, which the change's old side holds. */
export function linesChanged(
  hunks: readonly Hunk[],
  range: DiffSelection,
): boolean {
  return hunks.some((hunk) =>
    hunk.lines.some(
      (line) =>
        line.kind === "removed" &&
        line.old_no !== null &&
        line.old_no >= range.start_line &&
        line.old_no <= range.end_line,
    ),
  );
}

/** One line of a review comment's range as its version's change holds it. */
export interface RangeLine {
  n: number;
  text: string;
  kind: "added" | "removed" | "context";
}

/** The lines of `range` that the hunks of its version's full change hold, in order; lines outside every hunk are left out. */
export function rangeLines(
  hunks: readonly Hunk[],
  range: DiffSelection,
): RangeLine[] {
  return hunks.flatMap((hunk) =>
    hunk.lines.flatMap((line): RangeLine[] => {
      const n = range.side === "new" ? line.new_no : line.old_no;
      return n !== null && n >= range.start_line && n <= range.end_line
        ? [{ n, text: line.text.replace(/\r?\n$/, ""), kind: line.kind }]
        : [];
    }),
  );
}

/** The About card's line on the conversation: how many threads it holds and how many wait for the human's answer; null for a proposal nobody wrote about. */
export function conversationFact(
  threads: readonly ConversationThread[],
): { threads: string; waiting: string | null } | null {
  if (threads.length === 0) return null;
  const waiting = threads.filter((t) => t.state === "needs_answer").length;
  return {
    threads:
      threads.length === 1 ? "1 thread" : `${String(threads.length)} threads`,
    waiting:
      waiting === 0
        ? null
        : waiting === 1
          ? "1 needs your answer"
          : `${String(waiting)} need your answer`,
  };
}

/** The state chips the filter row shows: All, every state that holds a thread, and the chosen state even when it holds none. */
export function chipStates(
  options: readonly FilterOption<StateFilter>[],
): FilterOption<StateFilter>[] {
  return options.filter(
    (option) => option.value === "all" || option.count > 0 || option.selected,
  );
}

/** The words of a dropdown of the filter row: the chosen option, or the words for every option. */
export function chosenLabel<T>(
  options: readonly FilterOption<T>[],
  none: string,
): string {
  return options.find((option) => option.selected)?.label ?? none;
}
