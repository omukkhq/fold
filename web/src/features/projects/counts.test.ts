import { describe, expect, it } from "vitest";
import type { ProjectOverview } from "../../api/projects";
import { countsLine, lastChangeLine } from "./counts";

function project(over: Partial<ProjectOverview>): ProjectOverview {
  return {
    slug: "demo",
    name: "demo",
    available: true,
    counts: { backlog: 0, todo: 0, progress: 0, review: 0, done: 0 },
    agents: [],
    mainRevision: null,
    lastChangeAt: null,
    ...over,
  };
}

describe("countsLine", () => {
  it("lists the non-empty columns and the main head", () => {
    expect(
      countsLine(
        project({
          counts: { backlog: 6, todo: 3, progress: 4, review: 1, done: 3 },
          mainRevision: 8,
        }),
      ),
    ).toBe(
      "17 tasks · 4 in progress · 1 in review · 3 ready · 6 in backlog · main r8",
    );
  });

  it("says when every task is Done", () => {
    expect(
      countsLine(
        project({
          counts: { backlog: 0, todo: 0, progress: 0, review: 0, done: 31 },
          mainRevision: 42,
        }),
      ),
    ).toBe("31 tasks · all Done · main r42");
  });

  it("handles an empty board and a single task", () => {
    expect(countsLine(project({}))).toBe("0 tasks");
    expect(
      countsLine(
        project({
          counts: { backlog: 1, todo: 0, progress: 0, review: 0, done: 0 },
        }),
      ),
    ).toBe("1 task · 1 in backlog");
  });
});

describe("lastChangeLine", () => {
  it("says how long ago the project changed", () => {
    expect(lastChangeLine(project({ lastChangeAt: 1000 }), 13_000)).toBe(
      "changed 12 s ago",
    );
  });

  it("says when nothing has changed", () => {
    expect(lastChangeLine(project({}), 13_000)).toBe("no changes yet");
  });
});
