import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import { clearStash, dropStashEntry, listStash } from "./stash";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the stash routes", () => {
  it("list, drop one entry by its place and clear the stash", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/stash": { body: { entries: [] } },
      "DELETE /api/v1/projects/demo/stash/2": {
        body: { dropped: [], entries: 1 },
      },
      "DELETE /api/v1/projects/demo/stash": {
        body: { dropped: [], entries: 0 },
      },
    });
    expect(await listStash("demo")).toEqual({ entries: [] });
    expect((await dropStashEntry("demo", 2)).entries).toBe(1);
    expect((await clearStash("demo")).entries).toBe(0);
    expect(calls.map((call) => `${call.method} ${call.path}`)).toEqual([
      "GET /api/v1/projects/demo/stash",
      "DELETE /api/v1/projects/demo/stash/2",
      "DELETE /api/v1/projects/demo/stash",
    ]);
  });

  it("reject with the daemon's code", async () => {
    stubFetch({
      "DELETE /api/v1/projects/demo/stash/0": errorReply(
        400,
        "invalid_request",
      ),
    });
    await expect(dropStashEntry("demo", 0)).rejects.toSatisfy(
      (error: unknown) => isApiError(error, "invalid_request"),
    );
  });
});
