import type { LiveWorkspace, TaskDetail } from "@/api/tasks";
import { groupDependencies } from "./dependencies";
import { recordedCheckpoints } from "./liveStrip";

/** The note after the Work group's empty items while the task has no proposal yet. */
export const PROPOSAL_NOTE =
  "A proposal opens when the worker calls create_proposal; the card then moves to Review.";

export interface Gaps {
  /** The empty items, `no changesets`, in the group's order. */
  items: string[];
  /** A sentence after them, or null. */
  note: string | null;
}

/** One line for every empty item of a group: `Nothing yet: no changesets, no proposal.`; null when no item is empty. */
export function nothingText(gaps: Gaps): string | null {
  if (gaps.items.length === 0) {
    return null;
  }
  const line = `${gaps.items.join(", ")}.`;
  return gaps.note === null ? line : `${line} ${gaps.note}`;
}

/** The People group's empty items: only the implementation claim can be empty, as the other rows hold their own controls. */
export function peopleGaps(task: Pick<TaskDetail, "claims">): Gaps {
  return {
    items: task.claims.length === 0 ? ["nobody holds the claim"] : [],
    note: null,
  };
}

/** The Work group's empty items for a task that ends with a proposal: the workspace, the checkpoints, the changesets and the proposal. */
export function workGaps(
  task: Pick<
    TaskDetail,
    "workspace" | "checkpoints" | "changesets" | "proposal"
  >,
  live: LiveWorkspace | null,
): Gaps {
  const items: string[] = [];
  if (task.workspace === null) items.push("no workspace");
  if (recordedCheckpoints(task.checkpoints, live).length === 0) {
    items.push("no checkpoints");
  }
  if (task.changesets.length === 0) items.push("no changesets");
  if (task.proposal === null) items.push("no proposal");
  return { items, note: task.proposal === null ? PROPOSAL_NOTE : null };
}

/** The Links group's empty items: the blocker and both sides of the dependencies. */
export function linkGaps(
  task: Pick<TaskDetail, "blocker" | "dependencies">,
): Gaps {
  const groups = groupDependencies(task.dependencies);
  const items: string[] = [];
  if (task.blocker === null) items.push("no blocker");
  if (groups.needs.length === 0) items.push("no prerequisites");
  if (groups.neededBy.length === 0) items.push("no task waits for this one");
  return { items, note: null };
}
