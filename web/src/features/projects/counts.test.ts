import { describe, expect, it } from "vitest";
import type { ProjectOverview } from "../../api/projects";
import { countsLine, lastChangeLine, summaryLine } from "./counts";

function overview(over: Partial<ProjectOverview>): ProjectOverview {
  return {
    counts: { backlog: 0, todo: 0, in_progress: 0, review: 0, done: 0 },
    agents: [],
    head: null,
    last_change_at: null,
    paused_since: null,
    paused_by: null,
    attention: [],
    ...over,
  };
}

describe("countsLine", () => {
  it("lists the non-empty columns and the main head", () => {
    expect(
      countsLine(
        overview({
          counts: { backlog: 6, todo: 3, in_progress: 4, review: 1, done: 3 },
          head: "r8",
        }),
      ),
    ).toBe(
      "17 tasks · 4 in progress · 1 in review · 3 ready · 6 in backlog · main r8",
    );
  });

  it("says when every task is Done", () => {
    expect(
      countsLine(
        overview({
          counts: { backlog: 0, todo: 0, in_progress: 0, review: 0, done: 31 },
          head: "r42",
        }),
      ),
    ).toBe("31 tasks · all Done · main r42");
  });

  it("handles an empty board and a single task", () => {
    expect(countsLine(overview({}))).toBe("0 tasks");
    expect(
      countsLine(
        overview({
          counts: { backlog: 1, todo: 0, in_progress: 0, review: 0, done: 0 },
        }),
      ),
    ).toBe("1 task · 1 in backlog");
  });
});

describe("lastChangeLine", () => {
  it("says how long ago the project changed", () => {
    expect(lastChangeLine(overview({ last_change_at: 1000 }), 13_000)).toBe(
      "changed 12 s ago",
    );
  });

  it("says when nothing has changed", () => {
    expect(lastChangeLine(overview({}), 13_000)).toBe("no changes yet");
  });
});

describe("summaryLine", () => {
  const base = {
    slug: "demo",
    name: "demo",
    root: "/home/ana/demo",
    last_used_at: 0,
    removed_at: null,
    folder: "present" as const,
  };

  it("gives the counts of a project with an overview", () => {
    expect(
      summaryLine({ ...base, available: true, overview: overview({}) }, 0),
    ).toBe("0 tasks");
  });

  it("tells an open project without an overview from one that is not open", () => {
    expect(summaryLine({ ...base, available: true, overview: null }, 0)).toBe(
      "Could not read this project.",
    );
    expect(summaryLine({ ...base, available: false, overview: null }, 0)).toBe(
      "Could not open this project.",
    );
  });

  it("says when a person removed the project and that its data stays", () => {
    expect(
      summaryLine(
        { ...base, available: false, overview: null, removed_at: 1_000 },
        61_000,
      ),
    ).toBe("Removed from fold 1 min ago. Its data stays in its folder.");
  });
});
