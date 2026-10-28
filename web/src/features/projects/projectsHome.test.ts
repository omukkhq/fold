import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import type { ProjectSummary } from "@/api/projects";
import {
  addBackBlocker,
  addBackFailure,
  inFold,
  matchesSearch,
  projectActions,
  projectMark,
  removeFailure,
  showsRoot,
  shortPath,
  shownProjects,
} from "./projectsHome";

function project(
  slug: string,
  fields: Partial<ProjectSummary> = {},
): ProjectSummary {
  return {
    slug,
    name: slug,
    root: `/home/ana/code/${slug}`,
    available: true,
    last_used_at: 0,
    removed_at: null,
    folder: "present",
    overview: null,
    ...fields,
  };
}

const web = project("web", { name: "Web shop" });
const old = project("old", { removed_at: 5, available: false });
const away = project("away", { available: false, folder: "folder_missing" });

describe("shownProjects", () => {
  it("lists the projects in fold without a search", () => {
    expect(shownProjects([web, old, away], "  ").map((p) => p.slug)).toEqual([
      "web",
      "away",
    ]);
  });

  it("finds every project by name, slug or folder, the removed ones too", () => {
    const all = [web, old, away];
    expect(shownProjects(all, "SHOP").map((p) => p.slug)).toEqual(["web"]);
    expect(shownProjects(all, "old").map((p) => p.slug)).toEqual(["old"]);
    expect(shownProjects(all, "/home/ana/code").map((p) => p.slug)).toEqual([
      "web",
      "old",
      "away",
    ]);
    expect(shownProjects(all, "nothing")).toEqual([]);
  });

  it("matches a search that has spaces around it", () => {
    expect(matchesSearch(web, " web ")).toBe(true);
  });
});

describe("the card of a project", () => {
  it("marks a removed project and one whose folder is not as fold left it", () => {
    expect(projectMark(web)).toBeNull();
    expect(projectMark(old)).toBe("removed");
    expect(projectMark(away)).toBe("folder missing");
    expect(projectMark(project("bare", { folder: "fold_missing" }))).toBe(
      ".fold missing",
    );
    expect(projectMark(project("nodb", { folder: "database_missing" }))).toBe(
      "database missing",
    );
    // A removed project is marked removed, whatever is at its folder.
    expect(projectMark({ ...old, folder: "folder_missing" })).toBe("removed");
  });

  it("shows the folder in search results and for a marked project", () => {
    expect(showsRoot(web, "")).toBe(false);
    expect(showsRoot(web, "w")).toBe(true);
    expect(showsRoot(away, "")).toBe(true);
  });

  it("offers Remove for a project in fold and Add back for a removed one whose folder holds it", () => {
    expect(inFold(web)).toBe(true);
    expect(projectActions(web)).toEqual({ remove: true, addBack: false });
    expect(projectActions(away)).toEqual({ remove: true, addBack: false });
    expect(projectActions(old)).toEqual({ remove: false, addBack: true });
    expect(projectActions({ ...old, folder: "fold_missing" })).toEqual({
      remove: false,
      addBack: false,
    });
  });

  it("says why a removed project cannot come back from its card", () => {
    expect(addBackBlocker(web)).toBeNull();
    expect(addBackBlocker(old)).toBeNull();
    expect(addBackBlocker({ ...old, folder: "folder_missing" })).toBe(
      "Put the folder back to add the project back.",
    );
    expect(addBackBlocker({ ...old, folder: "fold_missing" })).toBe(
      "Run `fld init` in the folder to make a new project there.",
    );
  });
});

describe("failed writes", () => {
  it("words a removal refused while workers run", () => {
    const failure = removeFailure(
      new ApiError("project_workers_running", "", { workers: ["web-lead"] }),
    );
    expect(failure.code).toBe("project_workers_running");
    expect(failure.text).toContain("Pause the project's agents");
    expect(removeFailure(new ApiError("internal_error", "")).text).toBe(
      "The daemon failed. Try again.",
    );
  });

  it("words an add back the folder refuses", () => {
    expect(addBackFailure(new ApiError("project_unavailable", "")).text).toBe(
      "The folder does not hold this project now, so it stays removed.",
    );
    expect(addBackFailure(new ApiError("internal_error", "")).code).toBe(
      "internal_error",
    );
  });
});

describe("shortPath", () => {
  it("keeps the last two folders of a long path and leaves a short one whole", () => {
    expect(shortPath("/home/tanim/programming/nobs")).toBe(
      "…/programming/nobs",
    );
    expect(shortPath("/srv/nobs/.git")).toBe("/srv/nobs/.git");
    expect(shortPath("/srv/nobs")).toBe("/srv/nobs");
    expect(shortPath("/home/tanim/code/nobs/.git")).toBe("…/nobs/.git");
  });
});
