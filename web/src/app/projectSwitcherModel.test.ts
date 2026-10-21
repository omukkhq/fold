import { describe, expect, it } from "vitest";
import type { ProjectSummary } from "../api/projects";
import { RECENT_PROJECTS, recentProjects } from "./projectSwitcherModel";

function project(
  slug: string,
  lastUsedAt: number,
  name: string = slug,
): ProjectSummary {
  return {
    slug,
    name,
    available: true,
    last_used_at: lastUsedAt,
    overview: null,
  };
}

describe("recentProjects", () => {
  it("lists the most recently used projects first, at most five", () => {
    const projects = [
      project("a", 1),
      project("b", 7),
      project("c", 3),
      project("d", 9),
      project("e", 5),
      project("f", 2),
    ];
    expect(recentProjects(projects).map((entry) => entry.slug)).toEqual([
      "d",
      "b",
      "e",
      "c",
      "f",
    ]);
    expect(recentProjects(projects)).toHaveLength(RECENT_PROJECTS);
  });

  it("orders projects used at the same time by slug", () => {
    expect(
      recentProjects([project("b", 4), project("a", 4)]).map((e) => e.slug),
    ).toEqual(["a", "b"]);
  });

  it("names the slug only beside a name that another listed project shares", () => {
    expect(
      recentProjects([
        project("nobs", 4),
        project("nobs-2", 3, "nobs"),
        project("web", 2),
      ]),
    ).toEqual([
      { slug: "nobs", name: "nobs", detail: "nobs" },
      { slug: "nobs-2", name: "nobs", detail: "nobs-2" },
      { slug: "web", name: "web", detail: null },
    ]);
  });

  it("lists nothing for no projects", () => {
    expect(recentProjects([])).toEqual([]);
  });
});
