import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import { getProject, listProjects } from "./projects";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("projects", () => {
  it("lists the registered projects", async () => {
    stubFetch({
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
