import type { Board, BoardCard, TaskStatus } from "@/api/board";
import type { LiveWorkspace, TaskDetail } from "@/api/tasks";

const STATUSES: TaskStatus[] = [
  "backlog",
  "todo",
  "in_progress",
  "review",
  "done",
];

/** A board card as the daemon sends it, with nothing set. */
export function boardCard(
  task: string,
  over: Partial<BoardCard> = {},
): BoardCard {
  return {
    task,
    title: `Title of ${task}`,
    status: "todo",
    blocked: false,
    blocked_by: [],
    needs: [],
    blocks: 0,
    messages: 0,
    completed_at: null,
    labels: [],
    checklist: {
      steps_done: 0,
      steps_total: 0,
      linked_done: 0,
      linked_total: 0,
    },
    worker: null,
    workspace: null,
    human_review: null,
    attention: [],
    proposal: null,
    activity: null,
    ...over,
  };
}

/** The five columns in board order; each card sits in the column of its status and `total` counts them unless given. */
export function boardView(
  cards: BoardCard[],
  totals: Partial<Record<TaskStatus, number>> = {},
): Board {
  return {
    columns: STATUSES.map((status) => {
      const mine = cards.filter((card) => card.status === status);
      return { status, cards: mine, total: totals[status] ?? mine.length };
    }),
  };
}

export function taskView(
  task: string,
  over: Partial<TaskDetail> = {},
): TaskDetail {
  return {
    task,
    title: `Title of ${task}`,
    description: "",
    status: "todo",
    blocked: false,
    labels: [],
    checklists: [],
    dependencies: [],
    claims: [],
    attention: [],
    human_review: null,
    workspace: null,
    progress: {
      steps_done: 0,
      steps_total: 0,
      linked_done: 0,
      linked_total: 0,
    },
    archived_at: null,
    created_at: 1_000,
    created_by: "orchestrator",
    updated_at: 1_000,
    human_review_set_at: null,
    status_reason:
      "Calculated: no workspace yet and every prerequisite is Done",
    workspace_info: null,
    activity: null,
    changesets: [],
    proposal: null,
    integration: null,
    history: [],
    can_change_human_review: false,
    ...over,
  };
}

export function liveWorkspace(
  workspace: string,
  task: string,
  over: Partial<LiveWorkspace> = {},
): LiveWorkspace {
  return {
    workspace,
    task,
    worker: "frontend/ui",
    base: "r7",
    files: [],
    summary: "",
    last_activity_at: 1_000,
    checkpoints: [],
    proposal: null,
    stalled: false,
    ...over,
  };
}
