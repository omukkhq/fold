import type { ProjectCost, TaskCost } from "@/api/cost";

const HOUR = 60 * 60 * 1000;

/** A week of turns: a lead, two task workers on T-12 and T-3, a reviewer of T-12 and the review lead. `today` is the viewer's local midnight. */
export function projectCost(
  today: number,
  over: Partial<ProjectCost> = {},
): ProjectCost {
  return {
    since: null,
    total: { cost_micro_usd: 10_000_000, turns: 40, workers: 5 },
    first_turn_at: today - 2 * 24 * HOUR + 9 * HOUR,
    by_role: [
      { role: "area_lead", cost_micro_usd: 2_000_000, turns: 10, workers: 1 },
      { role: "review_lead", cost_micro_usd: 500_000, turns: 4, workers: 1 },
      { role: "task_worker", cost_micro_usd: 6_000_000, turns: 20, workers: 2 },
      { role: "reviewer", cost_micro_usd: 1_500_000, turns: 6, workers: 1 },
    ],
    by_area: [
      { area: "web", cost_micro_usd: 8_000_000, turns: 30, workers: 3 },
      { area: null, cost_micro_usd: 2_000_000, turns: 10, workers: 2 },
    ],
    by_model: [
      {
        model: "claude-sonnet-5",
        cost_micro_usd: 7_000_000,
        turns: 30,
        workers: 4,
      },
      { model: null, cost_micro_usd: 3_000_000, turns: 10, workers: 1 },
    ],
    workers: [
      {
        name: "web-worker",
        role: "task_worker",
        area: "web",
        state: "stopped",
        model: "claude-sonnet-5",
        turns: 14,
        cost_micro_usd: 4_000_000,
      },
      {
        name: "reviewer-1",
        role: "reviewer",
        area: null,
        state: "idle",
        model: null,
        turns: 6,
        cost_micro_usd: 1_500_000,
      },
    ],
    tasks: [
      {
        task: "T-12",
        title: "Add the login form",
        area: "web",
        build_micro_usd: 4_000_000,
        review_micro_usd: 1_500_000,
        total_micro_usd: 5_500_000,
        turns: 20,
      },
      {
        task: "T-3",
        title: "Fix the header",
        area: null,
        build_micro_usd: 2_000_000,
        review_micro_usd: 0,
        total_micro_usd: 2_000_000,
        turns: 6,
      },
    ],
    slots: [
      {
        start: today + 9 * HOUR,
        role: "task_worker",
        cost_micro_usd: 1_000_000,
      },
      {
        start: today + 9 * HOUR + 15 * 60 * 1000,
        role: "reviewer",
        cost_micro_usd: 500_000,
      },
      { start: today + 14 * HOUR, role: "area_lead", cost_micro_usd: 250_000 },
      {
        start: today - 2 * 24 * HOUR + 9 * HOUR,
        role: "task_worker",
        cost_micro_usd: 3_000_000,
      },
    ],
    slot_minutes: 15,
    ...over,
  };
}

export function taskCost(over: Partial<TaskCost> = {}): TaskCost {
  return {
    task: "T-12",
    build_micro_usd: 4_000_000,
    review_micro_usd: 1_000_000,
    total_micro_usd: 5_000_000,
    turns: 20,
    workers: [
      {
        name: "web-worker",
        role: "task_worker",
        part: "build",
        model: "claude-sonnet-5",
        proposals: [],
        turns: 14,
        cost_micro_usd: 4_000_000,
      },
      {
        name: "reviewer-1",
        role: "reviewer",
        part: "review",
        model: null,
        proposals: ["P-3", "P-4"],
        turns: 6,
        cost_micro_usd: 1_000_000,
      },
    ],
    ...over,
  };
}
