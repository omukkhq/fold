import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import {
  getProject,
  listProjects,
  pauseProject,
  resumeProject,
} from "./projects";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("projects", () => {
  it("lists the registered projects with the cost of the viewer's own days", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects": {
        body: {
          projects: [
            { slug: "demo", name: "demo", available: true },
            { slug: "archive", name: "archive", available: false },
          ],
        },
      },
    });
    expect((await listProjects()).map((p) => p.available)).toEqual([
      true,
      false,
    ]);
    const today = Number(calls[0]?.search.get("today"));
    const week = Number(calls[0]?.search.get("week"));
    expect(new Date(today).getHours()).toBe(0);
    expect(today).toBeLessThanOrEqual(Date.now());
    expect(new Date(week).getDate()).toBe(
      new Date(
        new Date(today).setDate(new Date(today).getDate() - 6),
      ).getDate(),
    );
    expect(calls[0]?.search.get("include_removed")).toBe("true");
  });

  it("pauses and resumes the agents of a project", async () => {
    const paused = { paused_since: 1_760_000_000_000, paused_by: "human:ana" };
    const calls = stubFetch({
      "POST /api/v1/projects/demo/pause": { body: paused },
      "POST /api/v1/projects/demo/resume": {
        body: { paused_since: null, paused_by: null },
      },
    });
    expect(await pauseProject("demo")).toEqual(paused);
    expect((await resumeProject("demo")).paused_since).toBeNull();
    expect(calls.map((call) => call.method)).toEqual(["POST", "POST"]);
  });

  it("reads one project by slug", async () => {
    stubFetch({
      "GET /api/v1/projects/demo": {
        body: { slug: "demo", name: "demo", available: true },
      },
    });
    expect((await getProject("demo")).slug).toBe("demo");
  });

  it.each([
    [404, "project_not_found"],
    [503, "project_unavailable"],
  ])("rejects a status %i with the code %s", async (status, code) => {
    stubFetch({ "GET /api/v1/projects/x": errorReply(status, code) });
    await expect(getProject("x")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, code),
    );
  });
});
