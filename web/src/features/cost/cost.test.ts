import { describe, expect, it } from "vitest";
import type { ProjectSummary } from "@/api/projects";
import { projectCost, taskCost, tokens } from "@/test/cost";
import {
  allProjectsCost,
  areaRows,
  barLabel,
  costChart,
  costTiles,
  isCostRange,
  isEmptyCost,
  modelRows,
  projectCostLines,
  rangeSince,
  roleRowName,
  roleRows,
  shareText,
  showsTick,
  taskCostSection,
  taskRows,
  tokensCard,
  tokensText,
  workerRows,
} from "./cost";

const NOW = new Date(2026, 9, 21, 15, 30).getTime();
const TODAY = new Date(2026, 9, 21).getTime();

describe("ranges", () => {
  it("start at the viewer's midnight, six days back, or never", () => {
    expect(rangeSince("today", NOW)).toBe(TODAY);
    expect(rangeSince("week", NOW)).toBe(new Date(2026, 9, 15).getTime());
    expect(rangeSince("all", NOW)).toBeNull();
    expect(isCostRange("week")).toBe(true);
    expect(isCostRange("month")).toBe(false);
  });

  it("round a share to a whole percent", () => {
    expect(shareText(1, 3)).toBe("33%");
    expect(shareText(5, 0)).toBe("0%");
  });
});

describe("costTiles", () => {
  it("sums the reviews of the review lead and the reviewers and names the first turn", () => {
    expect(costTiles(projectCost(TODAY))).toEqual([
      {
        label: "Total",
        amount: "$10.00",
        detail: "since Oct 19 · 40 turns",
        tokens: ["2.0M tokens", "160.0k output"],
      },
      {
        label: "Task workers",
        amount: "$6.00",
        detail: "60% · 2 workers",
        tokens: ["1.2M tokens", "96.0k output"],
      },
      {
        label: "Reviews",
        amount: "$2.00",
        detail: "20% · 2 workers",
        tokens: ["400.0k tokens", "32.0k output"],
      },
      {
        label: "Leads",
        amount: "$2.00",
        detail: "20% · 1 worker",
        tokens: ["400.0k tokens", "32.0k output"],
      },
    ]);
  });

  it("counts the supervised main agent among the leads", () => {
    const view = projectCost(TODAY);
    const withMain = projectCost(TODAY, {
      by_role: [
        ...view.by_role,
        {
          role: "main",
          cost_micro_usd: 1_000_000,
          tokens: tokens(200_000),
          turns: 3,
          workers: 1,
        },
      ],
    });
    expect(costTiles(withMain)[3]).toMatchObject({
      label: "Leads",
      amount: "$3.00",
      detail: "30% · 2 workers",
    });
    expect(roleRowName("main")).toBe("Main agent");
  });

  it("says when there is no turn", () => {
    const empty = projectCost(TODAY, {
      first_turn_at: null,
      total: { cost_micro_usd: 0, tokens: tokens(0), turns: 0, workers: 0 },
    });
    expect(costTiles(empty)[0]?.detail).toBe("no turns yet");
    expect(isEmptyCost(empty)).toBe(true);
    expect(isEmptyCost(projectCost(TODAY))).toBe(false);
  });
});

describe("costChart", () => {
  it("draws today's 24 hours with the role split of each", () => {
    const chart = costChart(projectCost(TODAY), "today", NOW);
    expect(chart.title).toBe("Cost by hour");
    expect(chart.bars).toHaveLength(24);
    const nine = chart.bars[9];
    expect(nine?.cost).toBe(1_500_000);
    expect(nine?.tokens).toBe(300_000);
    expect(nine?.roles).toEqual([
      { role: "task_worker", cost: 1_000_000, tokens: 200_000 },
      { role: "reviewer", cost: 500_000, tokens: 100_000 },
    ]);
    expect(nine?.height).toBe(1);
    expect(nine?.tick).toBe("09");
    expect(nine?.title).toBe("09:00 today");
    expect(chart.bars[14]?.height).toBeCloseTo(1 / 6);
    expect(chart.bars[0]?.cost).toBe(0);
    if (nine) {
      expect(barLabel(nine)).toBe("09:00 today: $1.50 · 300.0k tokens");
    }
  });

  it("measures the bars by tokens when asked", () => {
    const view = projectCost(TODAY, {
      slots: [
        {
          start: TODAY + 9 * 60 * 60 * 1000,
          role: "task_worker",
          cost_micro_usd: 1_000_000,
          tokens: 100,
        },
        {
          start: TODAY + 10 * 60 * 60 * 1000,
          role: "reviewer",
          cost_micro_usd: 100,
          tokens: 400,
        },
      ],
    });
    const chart = costChart(view, "today", NOW, "tokens");
    expect(chart.title).toBe("Tokens by hour");
    expect(chart.bars[9]?.height).toBe(0.25);
    expect(chart.bars[10]?.height).toBe(1);
    expect(costChart(view, "today", NOW).bars[9]?.height).toBe(1);
  });

  it("draws the last seven days by weekday", () => {
    const chart = costChart(projectCost(TODAY), "week", NOW);
    expect(chart.title).toBe("Cost by day");
    expect(chart.bars.map((bar) => bar.tick)).toEqual([
      "Thu",
      "Fri",
      "Sat",
      "Sun",
      "Mon",
      "Tue",
      "Wed",
    ]);
    expect(chart.bars.map((bar) => bar.cost)).toEqual([
      0, 0, 0, 0, 3_000_000, 0, 1_750_000,
    ]);
    expect(chart.bars[6]?.title).toBe("Wed Oct 21");
  });

  it("adds the hour slots of a long range into their days", () => {
    const chart = costChart(
      projectCost(TODAY, {
        slot_minutes: 60,
        slots: [
          {
            start: TODAY + 23 * 60 * 60 * 1000,
            role: "task_worker",
            cost_micro_usd: 700_000,
            tokens: 0,
          },
          {
            start: TODAY - 60 * 60 * 1000,
            role: "reviewer",
            cost_micro_usd: 300_000,
            tokens: 0,
          },
        ],
      }),
      "all",
      NOW,
    );
    expect(chart.bars.slice(-2).map((bar) => bar.cost)).toEqual([
      300_000, 700_000,
    ]);
  });

  it("draws all time from the first turn's day, by week when it is long", () => {
    const chart = costChart(projectCost(TODAY), "all", NOW);
    expect(chart.bars.map((bar) => bar.tick)).toEqual([
      "Oct 15",
      "Oct 16",
      "Oct 17",
      "Oct 18",
      "Oct 19",
      "Oct 20",
      "Oct 21",
    ]);
    const long = costChart(
      projectCost(TODAY, {
        first_turn_at: new Date(2026, 3, 1, 12).getTime(),
        slots: [
          {
            start: new Date(2026, 3, 2).getTime(),
            role: "area_lead",
            cost_micro_usd: 7,
            tokens: 0,
          },
        ],
      }),
      "all",
      NOW,
    );
    expect(long.title).toBe("Cost by week");
    expect(long.bars[0]?.title).toBe("Week of Apr 1");
    expect(long.bars[0]?.cost).toBe(7);
    const none = costChart(
      projectCost(TODAY, { first_turn_at: null, slots: [] }),
      "all",
      NOW,
    );
    expect(none.bars).toHaveLength(7);
    expect(none.bars.every((bar) => bar.height === 0)).toBe(true);
  });

  it("prints every third hour, each weekday and about eight ticks of a long range", () => {
    expect([0, 1, 3].map((index) => showsTick(index, 24))).toEqual([
      true,
      false,
      true,
    ]);
    expect(showsTick(5, 7)).toBe(true);
    expect([0, 3, 4].map((index) => showsTick(index, 30))).toEqual([
      true,
      false,
      true,
    ]);
  });
});

describe("breakdowns", () => {
  it("lists roles with a turn, the most expensive first", () => {
    const base = projectCost(TODAY);
    const view = {
      ...base,
      by_role: base.by_role.map((entry) =>
        entry.role === "review_lead"
          ? { ...entry, turns: 0, cost_micro_usd: 0 }
          : entry,
      ),
    };
    expect(roleRows(view)).toEqual([
      {
        key: "task_worker",
        name: "Task workers",
        count: "2 workers",
        amount: "$6.00",
        fill: 1,
      },
      {
        key: "area_lead",
        name: "Area leads",
        count: "1 worker",
        amount: "$2.00",
        fill: 2 / 6,
      },
      {
        key: "reviewer",
        name: "Reviewers",
        count: "1 worker",
        amount: "$1.50",
        fill: 1.5 / 6,
      },
    ]);
  });

  it("puts the reviews in one row of every area and names an unknown model", () => {
    expect(areaRows(projectCost(TODAY)).map((row) => row.name)).toEqual([
      "web",
      "Reviews (all areas)",
    ]);
    expect(modelRows(projectCost(TODAY))).toEqual([
      {
        key: "claude-sonnet-5",
        name: "claude-sonnet-5",
        count: "30 turns",
        amount: "$7.00",
        fill: 1,
      },
      {
        key: "",
        name: "Unknown model",
        count: "10 turns",
        amount: "$3.00",
        fill: 3 / 7,
      },
    ]);
    expect(areaRows(projectCost(TODAY, { by_area: [] }))).toEqual([]);
  });

  it("lists the tasks with a dash for no review and counts the rest", () => {
    const { rows, more } = taskRows(projectCost(TODAY), 1);
    expect(rows).toEqual([
      {
        task: "T-12",
        title: "Add the login form",
        area: "web",
        build: "$4.00",
        review: "$1.50",
        total: "$5.50",
        tokens: "1.1M",
      },
    ]);
    expect(more).toBe(1);
    const all = taskRows(projectCost(TODAY), null);
    expect(all.more).toBe(0);
    expect(all.rows[1]).toMatchObject({ area: "—", review: "—" });
  });

  it("keeps stopped workers and marks them", () => {
    expect(workerRows(projectCost(TODAY))).toEqual([
      {
        name: "web-worker",
        role: "Task worker",
        area: "web",
        model: "claude-sonnet-5",
        turns: "14",
        cost: "$4.00",
        tokens: "800.0k",
        output: "64.0k",
        ended: true,
      },
      {
        name: "reviewer-1",
        role: "Reviewer",
        area: "—",
        model: "—",
        turns: "6",
        cost: "$1.50",
        tokens: "300.0k",
        output: "24.0k",
        ended: false,
      },
    ]);
  });
});

describe("tokens", () => {
  it("read as thousands and millions", () => {
    expect(tokensText(0)).toBe("0");
    expect(tokensText(999)).toBe("999");
    expect(tokensText(1_000)).toBe("1.0k");
    expect(tokensText(64_546)).toBe("64.5k");
    expect(tokensText(999_949)).toBe("999.9k");
    expect(tokensText(999_950)).toBe("1.0M");
    expect(tokensText(12_345_678)).toBe("12.3M");
  });

  it("split into their four parts and the share the prompt cache held", () => {
    expect(tokensCard(projectCost(TODAY))).toEqual({
      total: "2.0M",
      parts: [
        { key: "cache_read", label: "Cache read", amount: "1.6M", fill: 0.8 },
        {
          key: "cache_write",
          label: "Cache write",
          amount: "200.0k",
          fill: 0.1,
        },
        { key: "output", label: "Output", amount: "160.0k", fill: 0.08 },
        { key: "input", label: "Input", amount: "40.0k", fill: 0.02 },
      ],
      cacheLine: "87% of the input came from the prompt cache",
    });
    const none = tokensCard(
      projectCost(TODAY, {
        total: { cost_micro_usd: 0, tokens: tokens(0), turns: 0, workers: 0 },
      }),
    );
    expect(none.parts.every((part) => part.fill === 0)).toBe(true);
    expect(none.cacheLine).toBeNull();
  });
});

describe("taskCostSection", () => {
  it("splits build and review and says what each worker did", () => {
    expect(taskCostSection(taskCost())).toEqual({
      total: "$5.00",
      tokens: "1.0M tokens · 800.0k from the cache · 80.0k output",
      buildPart: 0.8,
      build: "$4.00",
      review: "$1.00",
      rows: [
        {
          name: "web-worker",
          line: "task worker · claude-sonnet-5 · 14 turns · built the task",
          amount: "$4.00",
          tokens: "800.0k tokens",
        },
        {
          name: "reviewer-1",
          line: "reviewer · unknown model · 6 turns · reviewed P-3, P-4",
          amount: "$1.00",
          tokens: "200.0k tokens",
        },
      ],
    });
    const free = taskCostSection(
      taskCost({
        build_micro_usd: 0,
        review_micro_usd: 0,
        total_micro_usd: 0,
        workers: [
          {
            name: "reviewer-2",
            role: "reviewer",
            part: "review",
            model: "opus",
            proposals: [],
            turns: 1,
            cost_micro_usd: 0,
            tokens: tokens(0),
          },
        ],
      }),
    );
    expect(free.buildPart).toBe(0);
    expect(free.rows[0]?.line).toBe("reviewer · opus · 1 turn · reviewed");
  });
});

describe("project cost", () => {
  const cost = {
    all_time_micro_usd: 12_340_000,
    today_micro_usd: 1_200_000,
    week_micro_usd: 5_670_000,
    all_time_tokens: 1_234_000,
  };
  const project = (all: number | null): ProjectSummary => ({
    slug: "web",
    name: "web",
    root: "/home/ana/web",
    available: all !== null,
    last_used_at: 0,
    removed_at: null,
    folder: "present",
    overview:
      all === null
        ? null
        : {
            counts: { backlog: 0, todo: 0, in_progress: 0, review: 0, done: 0 },
            agents: [],
            head: null,
            last_change_at: null,
            attention: [],
            paused_since: null,
            paused_by: null,
            cost: { ...cost, all_time_micro_usd: all },
          },
  });

  it("shows the total with today and the week under it", () => {
    expect(projectCostLines(project(12_340_000).overview)).toEqual({
      total: "$12.34",
      recent: "today $1.20 · 7 days $5.67",
      tokens: "1.2M tokens",
    });
    expect(projectCostLines(null)).toBeNull();
  });

  it("adds up the projects the list could read", () => {
    expect(
      allProjectsCost([project(1_000_000), project(null), project(2_500_000)]),
    ).toEqual({ cost: "$3.50", tokens: "2.5M tokens" });
  });
});
