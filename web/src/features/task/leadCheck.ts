import type { TaskDetail, TaskReport } from "@/api/tasks";
import { formatSince } from "@/format/time";
import { actorSegment, seg, type Segment } from "@/people/segments";

/** Where the lead check of a task without a proposal stands, read from its newest report. */
export type LeadCheckState =
  | "not_reported"
  | "waiting"
  | "sent_back"
  | "completed";

export interface LeadCheck {
  state: LeadCheckState;
  title: string;
  /** The newest report; null before the task worker's first one. */
  report: TaskReport | null;
  /** A report waits on a task that is still on the board, so a human may complete it or send it back. */
  canCheck: boolean;
}

const TITLES: Record<LeadCheckState, string> = {
  not_reported: "Not reported yet",
  waiting: "Waits for a lead check",
  sent_back: "Sent back to the task worker",
  completed: "Completed by a lead check",
};

/** What the kind means, for the side group of the task. */
export const KIND_NOTE =
  "This task needs no proposal. Its task worker works with the tools the task names and reports the work; a lead check completes the task or sends it back.";

/** A `no_proposal` or `human` task has no workspace, so the workspace and proposal parts of the task are left out. */
export function needsProposal(task: Pick<TaskDetail, "kind">): boolean {
  return task.kind === "proposal";
}

/** The lead check of a `no_proposal` task; null for any other kind. */
export function leadCheck(
  task: Pick<TaskDetail, "kind" | "reports" | "archived_at">,
): LeadCheck | null {
  if (task.kind !== "no_proposal") {
    return null;
  }
  const report = task.reports[0] ?? null;
  let state: LeadCheckState;
  if (report === null) {
    state = "not_reported";
  } else if (report.resolution === null) {
    state = "waiting";
  } else {
    state = report.resolution;
  }
  return {
    state,
    title: TITLES[state],
    report,
    canCheck: state === "waiting" && task.archived_at === null,
  };
}

/** `Reported by @web-worker · 2 min ago`. */
export function reportedText(report: TaskReport, now: number): Segment[] {
  return seg`Reported by ${actorSegment(report.by)} · ${formatSince(now, report.at)}`;
}

/** `Sent back by @ana · 1 min ago`, or the completion; null while the report waits. */
export function resolvedText(
  report: TaskReport,
  now: number,
): Segment[] | null {
  if (
    report.resolution === null ||
    report.resolved_by === null ||
    report.resolved_at === null
  ) {
    return null;
  }
  const verb =
    report.resolution === "sent_back" ? "Sent back by" : "Completed by";
  return seg`${verb} ${actorSegment(report.resolved_by)} · ${formatSince(now, report.resolved_at)}`;
}
