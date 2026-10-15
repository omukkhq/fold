import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { comparedFile, comparison, fileDiff } from "@/test/diff";
import { errorReply, stubFetch, type StubRequest } from "@/test/stubFetch";
import { getComparison, getFileDiff } from "./diff";
import { isApiError } from "./errors";
import { keys } from "./keys";
import { comparisonQuery, fileDiffQuery } from "./queries";

afterEach(() => {
  vi.unstubAllGlobals();
});

const LOGIN = comparedFile("src/login.tsx", "modified");
const FILES = { view: "files", ...comparison([LOGIN]) };
const FILE = { view: "file", ...fileDiff(LOGIN, []) };

function query(calls: StubRequest[], path: string) {
  return calls
    .filter((c) => c.path === path)
    .map((c) => Object.fromEntries(c.search));
}

describe("getComparison", () => {
  it("asks the diff route for the two states and answers its file list", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/diff": { body: FILES },
    });
    const answer = await getComparison("demo", "r7", "ws-7");
    expect(answer?.files).toEqual([LOGIN]);
    expect(answer?.source).toBe("working");
    expect(query(calls, "/api/v1/projects/demo/diff")).toEqual([
      { from: "r7", to: "ws-7" },
    ]);
  });

  it("compares a workspace named alone with its base revision", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/workspaces/ws-7": { body: { base: "r5" } },
      "GET /api/v1/projects/demo/diff": { body: FILES },
    });
    await getComparison("demo", undefined, "ws-7");
    expect(query(calls, "/api/v1/projects/demo/diff")).toEqual([
      { from: "r5", to: "ws-7" },
    ]);
  });

  it("has nothing to compare for any other state named alone and asks nothing", async () => {
    const calls = stubFetch({});
    await expect(getComparison("demo", undefined, "C-4")).resolves.toBeNull();
    expect(calls).toEqual([]);
  });

  it("rejects with the daemon's code", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/diff": errorReply(400, "invalid_state_ref"),
    });
    await expect(getComparison("demo", "x", "ws-7")).rejects.toSatisfy(
      (e: unknown) => isApiError(e, "invalid_state_ref"),
    );
  });

  it("refuses an answer of the other shape", async () => {
    stubFetch({ "GET /api/v1/projects/demo/diff": { body: FILE } });
    await expect(getComparison("demo", "r7", "ws-7")).rejects.toSatisfy(
      (e: unknown) => isApiError(e, "internal_error"),
    );
  });
});

describe("getFileDiff", () => {
  it("asks for one path and answers its hunks", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/diff": { body: FILE },
    });
    const diff = await getFileDiff("demo", "r7", "ws-7", "src/login.tsx");
    expect(diff.path).toBe("src/login.tsx");
    expect(query(calls, "/api/v1/projects/demo/diff")).toEqual([
      { from: "r7", to: "ws-7", path: "src/login.tsx" },
    ]);
  });

  it("reads a workspace named alone against its base", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/workspaces/ws-7": { body: { base: "r5" } },
      "GET /api/v1/projects/demo/diff": { body: FILE },
    });
    await getFileDiff("demo", undefined, "ws-7", "src/login.tsx");
    expect(query(calls, "/api/v1/projects/demo/diff")[0]?.from).toBe("r5");
  });

  it("rejects a state named alone that has no base, and an answer of the other shape", async () => {
    stubFetch({ "GET /api/v1/projects/demo/diff": { body: FILES } });
    await expect(
      getFileDiff("demo", undefined, "ck-3", "a.ts"),
    ).rejects.toSatisfy((e: unknown) => isApiError(e, "invalid_state_ref"));
    await expect(getFileDiff("demo", "r7", "ws-7", "a.ts")).rejects.toSatisfy(
      (e: unknown) => isApiError(e, "internal_error"),
    );
  });

  it("rejects a path the comparison does not list with the daemon's code", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/diff": errorReply(
        404,
        "path_not_in_comparison",
      ),
    });
    await expect(
      getFileDiff("demo", "r7", "ws-7", "nope.ts"),
    ).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "path_not_in_comparison"),
    );
  });
});

describe("diff queries", () => {
  it("key a comparison and a file under the project with `to` at index 3", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/diff": (request) => ({
        body: request.search.has("path") ? FILE : FILES,
      }),
    });
    const client = new QueryClient();
    const list = comparisonQuery("demo", "r7", "r8");
    const file = fileDiffQuery("demo", "r7", "r8", "src/login.tsx");
    expect(list.queryKey).toEqual(keys.comparison("demo", "r7", "r8"));
    expect(comparisonQuery("demo", undefined, "ws-7").queryKey).toEqual([
      "demo",
      "comparison",
      null,
      "ws-7",
    ]);
    expect(file.queryKey).toEqual([
      "demo",
      "file-diff",
      "r7",
      "r8",
      "src/login.tsx",
    ]);
    const [c, f] = await Promise.all([client.query(list), client.query(file)]);
    expect(c?.files).toHaveLength(1);
    expect(f.path).toBe("src/login.tsx");
  });
});
