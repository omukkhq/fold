import type { ProjectCost, TaskCost } from "@/api/cost";
import type { ProjectOverview, ProjectSummary } from "@/api/projects";
import type { WorkerRole } from "@/api/workers";
import {
  addDays,
  dayText,
  hourOfDay,
  hourText,
  localDayStart,
  weekdayText,
} from "@/format/days";
import { costText } from "@/features/workers/team";

export { costText };

/** The ranges of the cost view, as the URL names them. */
export type CostRange = "today" | "week" | "all";

export const COST_RANGES: readonly { value: CostRange; label: string }[] = [
  { value: "today", label: "Today" },
  { value: "week", label: "Last 7 days" },
  { value: "all", label: "All time" },
];

export function isCostRange(value: unknown): value is CostRange {
  return value === "today" || value === "week" || value === "all";
}

/** The start of the range in UTC milliseconds: the viewer's local midnight today, local midnight six days ago, or `null` for all time. */
export function rangeSince(range: CostRange, now: number): number | null {
  switch (range) {
    case "today":
      return localDayStart(now);
    case "week":
      return addDays(localDayStart(now), -6);
    default:
      return null;
  }
}

function plural(count: number, noun: string): string {
  return `${String(count)} ${noun}${count === 1 ? "" : "s"}`;
}

/** `40%` of `total`; nothing spent is 0%. */
export function shareText(part: number, total: number): string {
  if (total === 0) return "0%";
  return `${String(Math.round((part / total) * 100))}%`;
}

/** The part of `max` a bar fills, from 0 to 1. */
function fraction(part: number, max: number): number {
  return max === 0 ? 0 : part / max;
}

export interface CostTile {
  label: string;
  amount: string;
  detail: string;
}

/** The four totals: everything, then the task workers, the reviews (the review lead and the reviewers) and the area leads, each with its share and its workers. */
export function costTiles(view: ProjectCost): CostTile[] {
  const total = view.total.cost_micro_usd;
  const sum = (roles: readonly WorkerRole[]) =>
    view.by_role
      .filter((entry) => roles.includes(entry.role))
      .reduce(
        (acc, entry) => ({
          cost: acc.cost + entry.cost_micro_usd,
          workers: acc.workers + entry.workers,
        }),
        { cost: 0, workers: 0 },
      );
  const part = (label: string, roles: readonly WorkerRole[]): CostTile => {
    const { cost, workers } = sum(roles);
    return {
      label,
      amount: costText(cost),
      detail: `${shareText(cost, total)} · ${plural(workers, "worker")}`,
    };
  };
  return [
    {
      label: "Total",
      amount: costText(total),
      detail:
        view.first_turn_at === null
          ? "no turns yet"
          : `since ${dayText(view.first_turn_at)} · ${plural(view.total.turns, "turn")}`,
    },
    part("Task workers", ["task_worker"]),
    part("Reviews", ["review_lead", "reviewer"]),
    part("Leads", ["area_lead"]),
  ];
}

const ROLE_ROWS: Record<WorkerRole, string> = {
  area_lead: "Area leads",
  review_lead: "Review lead",
  task_worker: "Task workers",
  reviewer: "Reviewers",
};

/** Every role a turn can have, in the order the chart's tooltip lists them. */
export const CHART_ROLES: readonly WorkerRole[] = [
  "task_worker",
  "reviewer",
  "review_lead",
  "area_lead",
];

export function roleRowName(role: WorkerRole): string {
  return ROLE_ROWS[role];
}

export interface CostBar {
  /** The start of the bar's hour or day, in UTC milliseconds. */
  start: number;
  /** The short tick under the bar. */
  tick: string;
  /** The bar's hour or day in words, for its tooltip. */
  title: string;
  total: number;
  /** Each role with a turn in the bar, in [`CHART_ROLES`] order. */
  roles: { role: WorkerRole; cost: number }[];
  /** The part of the tallest bar this one fills, from 0 to 1. */
  height: number;
}

export interface CostChart {
  title: string;
  bars: CostBar[];
}

/** The most days a chart draws a bar for each; a longer range draws a bar for each week. */
const MAX_DAY_BARS = 92;

/** The starts of the chart's bars: the hours of today, the seven days of the week, or every day (or week) from the first turn to today, and at least the last seven days. */
function binStarts(
  range: CostRange,
  now: number,
  first: number | null,
): { starts: number[]; unit: "hour" | "day" | "week" } {
  const today = localDayStart(now);
  if (range === "today") {
    return {
      starts: Array.from({ length: 24 }, (_, hour) => hourOfDay(today, hour)),
      unit: "hour",
    };
  }
  const week = addDays(today, -6);
  const firstDay =
    range === "week" || first === null
      ? week
      : Math.min(localDayStart(first), week);
  const days: number[] = [];
  for (let day = firstDay; day <= today; day = addDays(day, 1)) {
    days.push(day);
  }
  if (days.length <= MAX_DAY_BARS) {
    return { starts: days, unit: "day" };
  }
  return {
    starts: days.filter((_, index) => index % 7 === 0),
    unit: "week",
  };
}

/** The cost by hour (today) or by day, split by role, in the viewer's own hours and days. A slot belongs to the last bar that starts at or before it. */
export function costChart(
  view: ProjectCost,
  range: CostRange,
  now: number,
): CostChart {
  const { starts, unit } = binStarts(range, now, view.first_turn_at);
  const sums = starts.map(() => new Map<WorkerRole, number>());
  for (const slot of view.slots) {
    let index = -1;
    for (let at = 0; at < starts.length; at += 1) {
      if ((starts[at] ?? Infinity) <= slot.start) index = at;
    }
    const bin = sums[index];
    if (bin) {
      bin.set(slot.role, (bin.get(slot.role) ?? 0) + slot.cost_micro_usd);
    }
  }
  const totals = sums.map((bin) =>
    [...bin.values()].reduce((sum, cost) => sum + cost, 0),
  );
  const max = Math.max(0, ...totals);
  const bars = starts.map((start, index) => {
    const bin = sums[index] ?? new Map<WorkerRole, number>();
    const total = totals[index] ?? 0;
    return {
      start,
      tick:
        unit === "hour"
          ? hourText(start).slice(0, 2)
          : unit === "day" && range === "week"
            ? weekdayText(start)
            : dayText(start),
      title:
        unit === "hour"
          ? `${hourText(start)} today`
          : unit === "week"
            ? `Week of ${dayText(start)}`
            : `${weekdayText(start)} ${dayText(start)}`,
      total,
      roles: CHART_ROLES.flatMap((role) => {
        const cost = bin.get(role) ?? 0;
        return cost > 0 ? [{ role, cost }] : [];
      }),
      height: fraction(total, max),
    };
  });
  return {
    title:
      unit === "hour"
        ? "Cost by hour"
        : unit === "week"
          ? "Cost by week"
          : "Cost by day",
    bars,
  };
}

/** Which ticks the chart prints: every third hour, every day of a week, and about eight ticks of a longer range. */
export function showsTick(index: number, count: number): boolean {
  if (count <= 7) return true;
  if (count === 24) return index % 3 === 0;
  return index % Math.ceil(count / 8) === 0;
}

export interface BreakdownRow {
  key: string;
  name: string;
  count: string;
  amount: string;
  /** The part of the largest row this one fills, from 0 to 1. */
  fill: number;
}

function rows<T extends { cost_micro_usd: number }>(
  entries: readonly T[],
  describe: (entry: T) => { key: string; name: string; count: string },
): BreakdownRow[] {
  const max = Math.max(0, ...entries.map((entry) => entry.cost_micro_usd));
  return entries.map((entry) => ({
    ...describe(entry),
    amount: costText(entry.cost_micro_usd),
    fill: fraction(entry.cost_micro_usd, max),
  }));
}

/** The roles with a turn, the most expensive first. */
export function roleRows(view: ProjectCost): BreakdownRow[] {
  return rows(
    [...view.by_role]
      .filter((entry) => entry.turns > 0)
      .sort((a, b) => b.cost_micro_usd - a.cost_micro_usd),
    (entry) => ({
      key: entry.role,
      name: roleRowName(entry.role),
      count: plural(entry.workers, "worker"),
    }),
  );
}

/** The areas, with the review lead and the reviewers in one row: they review every area's work. */
export function areaRows(view: ProjectCost): BreakdownRow[] {
  return rows(view.by_area, (entry) => ({
    key: entry.area ?? "",
    name: entry.area ?? "Reviews (all areas)",
    count: plural(entry.workers, "worker"),
  }));
}

export function modelRows(view: ProjectCost): BreakdownRow[] {
  return rows(view.by_model, (entry) => ({
    key: entry.model ?? "",
    name: entry.model ?? "Unknown model",
    count: plural(entry.turns, "turn"),
  }));
}

/** A cell without a value. */
export const NO_VALUE = "—";

export interface TaskRow {
  task: string;
  title: string;
  area: string;
  build: string;
  review: string;
  total: string;
}

/** The most expensive tasks first, at most `limit` of them, and how many more there are. A task without a review cost shows no review amount. */
export function taskRows(
  view: ProjectCost,
  limit: number | null,
): { rows: TaskRow[]; more: number } {
  const shown = limit === null ? view.tasks : view.tasks.slice(0, limit);
  return {
    rows: shown.map((task) => ({
      task: task.task,
      title: task.title,
      area: task.area ?? NO_VALUE,
      build: costText(task.build_micro_usd),
      review:
        task.review_micro_usd === 0
          ? NO_VALUE
          : costText(task.review_micro_usd),
      total: costText(task.total_micro_usd),
    })),
    more: view.tasks.length - shown.length,
  };
}

export interface WorkerRow {
  name: string;
  role: string;
  area: string;
  model: string;
  turns: string;
  cost: string;
  /** The worker has no process now, but its turns stay counted. */
  ended: boolean;
}

const ONE_ROLE: Record<WorkerRole, string> = {
  area_lead: "Area lead",
  review_lead: "Review lead",
  task_worker: "Task worker",
  reviewer: "Reviewer",
};

export function workerRows(view: ProjectCost): WorkerRow[] {
  return view.workers.map((worker) => ({
    name: worker.name,
    role: ONE_ROLE[worker.role],
    area: worker.area ?? NO_VALUE,
    model: worker.model ?? NO_VALUE,
    turns: String(worker.turns),
    cost: costText(worker.cost_micro_usd),
    ended: worker.state === "stopped" || worker.state === "failed",
  }));
}

/** The view has no turn in its range. */
export function isEmptyCost(view: ProjectCost): boolean {
  return view.total.turns === 0;
}

export interface TaskCostSection {
  total: string;
  /** The part of the total its build is, from 0 to 1; 0 for a task that cost nothing. */
  buildPart: number;
  build: string;
  review: string;
  rows: { name: string; line: string; amount: string }[];
}

/** The task screen's cost section: the total, the build and review parts and one row per worker, with what it did. */
export function taskCostSection(cost: TaskCost): TaskCostSection {
  return {
    total: costText(cost.total_micro_usd),
    buildPart: fraction(cost.build_micro_usd, cost.total_micro_usd),
    build: costText(cost.build_micro_usd),
    review: costText(cost.review_micro_usd),
    rows: cost.workers.map((worker) => {
      const did =
        worker.part === "build"
          ? "built the task"
          : worker.proposals.length > 0
            ? `reviewed ${worker.proposals.join(", ")}`
            : "reviewed";
      return {
        name: worker.name,
        line: [
          ONE_ROLE[worker.role].toLowerCase(),
          worker.model ?? "unknown model",
          plural(worker.turns, "turn"),
          did,
        ].join(" · "),
        amount: costText(worker.cost_micro_usd),
      };
    }),
  };
}

/** A project card's cost: the total of all time, and today and the last seven days under it. */
export function projectCostLines(
  overview: ProjectOverview | null,
): { total: string; recent: string } | null {
  if (overview === null) return null;
  const { cost } = overview;
  return {
    total: costText(cost.all_time_micro_usd),
    recent: `today ${costText(cost.today_micro_usd)} · 7 days ${costText(cost.week_micro_usd)}`,
  };
}

/** The total of every project whose overview the list read. */
export function allProjectsCost(projects: readonly ProjectSummary[]): string {
  return costText(
    projects.reduce(
      (sum, project) => sum + (project.overview?.cost.all_time_micro_usd ?? 0),
      0,
    ),
  );
}
