import type { TaskDetail, TaskKind } from "@/api/tasks";
import { formatSince } from "@/format/time";
import { actorSegment, seg, type Segment } from "@/people/segments";

/** Where a human task stands for the human who reads it. */
export type HumanWorkState = "open" | "waits" | "done" | "archived";

export interface HumanWork {
  state: HumanWorkState;
  note: string;
  /** The prerequisites that are not Done, in the order the task lists them. */
  waitsFor: string[];
  /** The human may mark the work Done now. */
  canMarkDone: boolean;
}

function idList(ids: string[]): string {
  if (ids.length <= 1) {
    return ids.join("");
  }
  return `${ids.slice(0, -1).join(", ")} and ${ids[ids.length - 1] ?? ""}`;
}

/** The Done control of a human task; null for a task an agent works on. */
export function humanWork(
  task: Pick<
    TaskDetail,
    "kind" | "status" | "archived_at" | "dependencies" | "can_mark_done"
  >,
): HumanWork | null {
  if (task.kind !== "human") {
    return null;
  }
  const waitsFor = task.dependencies
    .filter((edge) => edge.direction === "needs" && edge.status !== "done")
    .map((edge) => edge.task);
  let state: HumanWorkState;
  let note: string;
  if (task.archived_at !== null) {
    state = "archived";
    note = "A person does this work outside fold. The task is archived.";
  } else if (task.status === "done") {
    state = "done";
    note =
      "A person did this work outside fold, and it is marked Done. The tasks that need it are unblocked.";
  } else if (waitsFor.length > 0) {
    state = "waits";
    note = `A person does this work outside fold. Mark it Done once ${idList(waitsFor)} ${waitsFor.length === 1 ? "is" : "are"} Done.`;
  } else {
    state = "open";
    note =
      "A person does this work outside fold. No agent claims it. Mark it Done when the work is finished; the tasks that need it unblock.";
  }
  return {
    state,
    note,
    waitsFor,
    canMarkDone: state === "open" && task.can_mark_done,
  };
}

export interface BlockerView {
  reason: string;
  /** `Set by @web-lead · 2 min ago`. */
  setBy: Segment[];
}

/** The blocker of the task as the side group shows it; null without one. */
export function blockerView(
  task: Pick<TaskDetail, "blocker">,
  now: number,
): BlockerView | null {
  const blocker = task.blocker;
  if (blocker === null) {
    return null;
  }
  return {
    reason: blocker.reason,
    setBy: seg`Set by ${actorSegment(blocker.set_by)} · ${formatSince(now, blocker.set_at)}`,
  };
}

/** The daemon decides both: a named human sets or changes a blocker only before an agent starts the task, and clears one on a task that is not archived. */
export function blockerControls(
  task: Pick<TaskDetail, "can_set_blocker" | "can_clear_blocker">,
): { canSet: boolean; canClear: boolean } {
  return { canSet: task.can_set_blocker, canClear: task.can_clear_blocker };
}

export interface KindOption {
  value: TaskKind;
  title: string;
  hint: string;
}

/** The choices of the new task form, the daemon's default first. */
export const KIND_OPTIONS: KindOption[] = [
  {
    value: "proposal",
    title: "An agent, with a proposal",
    hint: "The coder changes files and sends a proposal; integration completes the task.",
  },
  {
    value: "no_proposal",
    title: "An agent, with no proposal",
    hint: "For work that changes no project file, such as screens drawn through an MCP server. The coder reports it, and a lead check completes it.",
  },
  {
    value: "human",
    title: "A person",
    hint: "Work only a person can do outside fold. No agent takes it; a human marks it Done.",
  },
];
