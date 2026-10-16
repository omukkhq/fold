import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import { getProjectGit } from "./git";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("getProjectGit", () => {
  it("reads the repository, the branch and the remotes", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/git": {
        body: { repository: "/home/ana/ledger", branch: "main", remotes: [] },
      },
    });
    expect(await getProjectGit("demo")).toEqual({
      repository: "/home/ana/ledger",
      branch: "main",
      remotes: [],
    });
  });

  it("rejects with the daemon's code", async () => {
    stubFetch({
      "GET /api/v1/projects/x/git": errorReply(404, "project_not_found"),
    });
    await expect(getProjectGit("x")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "project_not_found"),
    );
  });
});
