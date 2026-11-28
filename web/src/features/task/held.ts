import type { TaskDetail } from "@/api/tasks";

/** The held tasks a task waits on, each once and in task-number order; empty when the daemon names none. */
export function heldTasks(task: Pick<TaskDetail, "held_by">): string[] {
  const ids = [...new Set(task.held_by ?? [])];
  return ids.sort((a, b) => Number(a.slice(2)) - Number(b.slice(2)));
}
