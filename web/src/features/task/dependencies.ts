import type { TaskStatus } from "@/api/board";
import { apiErrorCode, ApiError } from "@/api/errors";
import type { TaskEdge } from "@/api/tasks";

export interface DependencyRow extends TaskEdge {
  /** The other task is not Done yet. */
  unfinished: boolean;
  /** Only a direct prerequisite edge can be removed here; a linked item's edge goes with the item. */
  removable: boolean;
}

export interface DependencyGroups {
  needs: DependencyRow[];
  neededBy: DependencyRow[];
  /** Unfinished prerequisites: the tasks this one waits for. */
  waitingFor: number;
}

function byNumber(a: TaskEdge, b: TaskEdge): number {
  return Number(a.task.slice(2)) - Number(b.task.slice(2));
}

/** Prerequisites and dependents apart, each in task-number order, with the unfinished ones flagged. */
export function groupDependencies(
  edges: readonly TaskEdge[],
): DependencyGroups {
  const rows = [...edges].sort(byNumber).map((edge) => ({
    ...edge,
    unfinished: edge.status !== "done",
    removable: edge.direction === "needs" && edge.origin === "direct",
  }));
  const needs = rows.filter((row) => row.direction === "needs");
  return {
    needs,
    neededBy: rows.filter((row) => row.direction === "needed_by"),
    waitingFor: needs.filter((row) => row.unfinished).length,
  };
}

/** A Done task takes no new dependencies, so it shows no editor. */
export function canEditDependencies(status: TaskStatus): boolean {
  return status !== "done";
}

function isTaskIds(value: unknown): value is string[] {
  return (
    Array.isArray(value) &&
    value.length > 0 &&
    value.every((v) => typeof v === "string")
  );
}

/** The task ids in `details.path` of a `dependency_cycle` refusal, first task repeated at the end; null for any other error. */
export function cyclePath(error: unknown): string[] | null {
  if (
    apiErrorCode(error) !== "dependency_cycle" ||
    !(error instanceof ApiError)
  ) {
    return null;
  }
  const { path } = error.details;
  return isTaskIds(path) ? path : null;
}

export function cycleText(path: readonly string[]): string {
  return `The edge would close a cycle: ${path.join(" → ")}.`;
}

/** `Needs · 1 unfinished`, so the waiting is read without scanning the rows. */
export function needsTitle(groups: DependencyGroups): string {
  return groups.waitingFor === 0
    ? "Needs"
    : `Needs · ${String(groups.waitingFor)} unfinished`;
}
