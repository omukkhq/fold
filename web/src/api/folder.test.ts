import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import { resetFolder, restorePaths } from "./folder";

afterEach(() => {
  vi.unstubAllGlobals();
});

const result = {
  revision: "r4",
  at_head: true,
  files: ["plan.md"],
  stashed: true,
  entries: 1,
  unchanged: [],
  dropped_unstash: [],
  dropped_change: null,
  skipped: [],
  unstash: null,
};

describe("the folder routes", () => {
  it("reset the folder and restore named paths", async () => {
    const calls = stubFetch({
      "POST /api/v1/projects/demo/folder/reset": { body: result },
      "POST /api/v1/projects/demo/folder/restore": { body: result },
    });
    expect((await resetFolder("demo")).stashed).toBe(true);
    expect((await restorePaths("demo", ["plan.md"])).files).toEqual([
      "plan.md",
    ]);
    expect(calls.map((call) => call.body)).toEqual([
      undefined,
      { paths: ["plan.md"] },
    ]);
  });

  it("reject with the daemon's code", async () => {
    stubFetch({
      "POST /api/v1/projects/demo/folder/reset": errorReply(
        400,
        "invalid_request",
      ),
    });
    await expect(resetFolder("demo")).rejects.toSatisfy((error: unknown) =>
      isApiError(error, "invalid_request"),
    );
  });
});
