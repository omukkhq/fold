import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { ApiError } from "../api/errors";
import { keys } from "../api/keys";
import {
  PROJECT_MISSING,
  forgetProject,
  isProjectGone,
  isProjectMissing,
  projectProblem,
} from "./projectGone";

describe("a project fold removed", () => {
  it("is told apart from every other failure by its code", () => {
    expect(isProjectGone(new ApiError("project_not_found", "gone"))).toBe(true);
    expect(isProjectGone(new ApiError("project_unavailable", "down"))).toBe(
      false,
    );
    expect(isProjectGone(new Error("project_not_found"))).toBe(false);
    expect(isProjectGone(null)).toBe(false);
  });

  it("names why a registered project cannot open, by the reason the daemon gives", () => {
    const unavailable = (details: Record<string, unknown>) =>
      projectProblem(new ApiError("project_unavailable", "down", details));
    expect(projectProblem(new ApiError("project_not_found", "gone"))).toEqual({
      kind: "gone",
    });
    expect(unavailable({ reason: "folder_missing", hint: "mount it" })).toEqual(
      { kind: "folder_missing", hint: "mount it" },
    );
    expect(unavailable({ reason: "database_missing" })).toEqual({
      kind: "database_missing",
      hint: null,
    });
    expect(unavailable({ reason: "old_layout", hint: 3 })).toEqual({
      kind: "unavailable",
      hint: null,
    });
    expect(
      projectProblem(
        new ApiError("project_not_found", "removed", {
          reason: "removed",
          hint: "run `fld claude` in /srv/app",
        }),
      ),
    ).toEqual({ kind: "removed", hint: "run `fld claude` in /srv/app" });
    expect(
      projectProblem(
        new ApiError("project_not_found", "removed", { reason: "removed" }),
      ),
    ).toEqual({ kind: "removed", hint: null });
    expect(projectProblem(new ApiError("task_not_found", "no"))).toBeNull();
    expect(projectProblem(new Error("project_unavailable"))).toBeNull();
  });

  it("loses its cached data, marks the list stale and reloads the route", () => {
    const queryClient = new QueryClient();
    queryClient.setQueryData(keys.project("nobs"), { slug: "nobs" });
    queryClient.setQueryData(keys.board("nobs"), { columns: [] });
    queryClient.setQueryData(keys.project("demo"), { slug: "demo" });
    queryClient.setQueryData(keys.projects(), { projects: [] });
    const reload = vi.fn();

    forgetProject(queryClient, "nobs", reload);

    expect(queryClient.getQueryData(keys.project("nobs"))).toBeUndefined();
    expect(queryClient.getQueryData(keys.board("nobs"))).toBeUndefined();
    expect(queryClient.getQueryData(keys.project("demo"))).toEqual({
      slug: "demo",
    });
    expect(queryClient.getQueryState(keys.projects())?.isInvalidated).toBe(
      true,
    );
    expect(reload).toHaveBeenCalledOnce();
  });
});

describe("the project route's not-found", () => {
  it("answers only for the project, never for a not-found that a screen inside it throws", () => {
    expect(isProjectMissing(PROJECT_MISSING)).toBe(true);
    expect(isProjectMissing(undefined)).toBe(false);
    expect(isProjectMissing(null)).toBe(false);
    expect(isProjectMissing("missing")).toBe(false);
    expect(isProjectMissing({ project: "other" })).toBe(false);
  });
});
