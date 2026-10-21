import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import {
  getProject,
  listProjects,
  pauseProject,
  removeProject,
  restoreProject,
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
    const today = new Date(2026, 9, 21).getTime();
    expect((await listProjects(today)).map((p) => p.available)).toEqual([
      true,
      false,
    ]);
    expect(Number(calls[0]?.search.get("today"))).toBe(today);
    expect(Number(calls[0]?.search.get("week"))).toBe(
      new Date(2026, 9, 15).getTime(),
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

  it("reads, removes and restores one project with the cost of the viewer's own days", async () => {
    const project = { slug: "demo", name: "demo", available: true };
    const calls = stubFetch({
      "GET /api/v1/projects/demo": { body: project },
      "DELETE /api/v1/projects/demo": { body: project },
      "POST /api/v1/projects/demo/restore": { body: project },
    });
    expect((await getProject("demo")).slug).toBe("demo");
    expect((await removeProject("demo")).slug).toBe("demo");
    expect((await restoreProject("demo")).slug).toBe("demo");
    expect(calls.map((call) => call.method)).toEqual(["GET", "DELETE", "POST"]);
    for (const call of calls) {
      expect(new Date(Number(call.search.get("today"))).getHours()).toBe(0);
      expect(Number(call.search.get("week"))).toBeLessThan(
        Number(call.search.get("today")),
      );
    }
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
