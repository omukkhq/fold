import { afterEach, describe, expect, it, vi } from "vitest";
import { stubFetch } from "@/test/stubFetch";
import {
  ignoreAgentChanges,
  recordAgentChanges,
  restoreAgentChanges,
} from "./agentChanges";
import { ApiError } from "./errors";

afterEach(() => {
  vi.unstubAllGlobals();
});

const BASE = "/api/v1/projects/demo/agent-changes";

const recorded = {
  outcome: "recorded",
  commit: null,
  revision: "r4",
  files: ["README.md"],
  conflicted: [],
  touched: [],
  skipped: [],
  created: true,
  unstash: null,
};

describe("agent changes", () => {
  it("records without and then with the confirmed tree", async () => {
    const calls = stubFetch({ [`POST ${BASE}/record`]: { body: recorded } });
    expect((await recordAgentChanges("demo")).revision).toBe("r4");
    expect(calls[0]?.search.has("confirm")).toBe(false);
    await recordAgentChanges("demo", "abc");
    expect(calls[1]?.search.get("confirm")).toBe("abc");
  });

  it("carries the details of a record that waits for a confirmation", async () => {
    stubFetch({
      [`POST ${BASE}/record`]: {
        status: 409,
        body: {
          error: {
            code: "record_needs_confirmation",
            message: "the change empties 1 tracked file",
            details: { tree: "abc", emptied: ["README.md"] },
          },
        },
      },
    });
    const refused = await recordAgentChanges("demo").catch(
      (error: unknown) => error,
    );
    expect(refused).toBeInstanceOf(ApiError);
    expect(refused instanceof ApiError && refused.details.tree).toBe("abc");
  });

  it("restores and ignores the files workers changed", async () => {
    stubFetch({
      [`POST ${BASE}/restore`]: {
        body: { revision: "r3", files: ["junk1.txt"], entries: 1 },
      },
      [`POST ${BASE}/ignore`]: { body: { files: ["notes.txt"] } },
    });
    expect((await restoreAgentChanges("demo")).files).toEqual(["junk1.txt"]);
    expect((await ignoreAgentChanges("demo")).files).toEqual(["notes.txt"]);
  });
});
