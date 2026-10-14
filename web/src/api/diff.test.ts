import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { comparisons } from "../../fixtures/diff";
import { getComparison, getFileDiff, pairKey } from "./diff";
import { isApiError } from "./errors";
import { keys } from "./keys";
import { comparisonQuery, fileDiffQuery } from "./queries";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

async function settle<T>(promise: Promise<T>): Promise<T> {
  await vi.runAllTimersAsync();
  return promise;
}

describe("the diff fixture", () => {
  const working = comparisons[pairKey("r7", "ws-7")];

  it("holds an added, a modified, a deleted, a renamed and a binary file", () => {
    const files = working?.comparison.files ?? [];
    const kinds = new Set(files.map((f) => f.change));
    expect([...kinds].sort()).toEqual([
      "added",
      "deleted",
      "modified",
      "renamed",
    ]);
    expect(files.some((f) => f.binary)).toBe(true);
  });

  it("has a diff for every file and its line counts match the hunks", () => {
    for (const entry of Object.values(comparisons)) {
      for (const file of entry.comparison.files) {
        const diff = entry.diffs.find((d) => d.path === file.path);
        expect(diff, file.path).toBeDefined();
        const lines = diff?.hunks.flatMap((h) => h.lines) ?? [];
        expect(lines.filter((l) => l.kind === "add")).toHaveLength(
          file.added_lines,
        );
        expect(lines.filter((l) => l.kind === "remove")).toHaveLength(
          file.removed_lines,
        );
        expect(diff?.binary).toBe(file.binary);
      }
    }
  });
});

describe("getComparison", () => {
  it("answers the comparison of two states", async () => {
    const comparison = await settle(getComparison("demo", "r7", "ws-7"));
    expect(comparison?.to).toMatchObject({
      kind: "workspace",
      id: "ws-7",
      base: "r7",
    });
  });

  it("compares to alone with its recorded base, as from and to together do", async () => {
    const alone = await settle(getComparison("demo", undefined, "ws-7"));
    const pair = await settle(getComparison("demo", "r7", "ws-7"));
    expect(alone).toEqual(pair);
  });

  it("answers null for two states it has no comparison of", async () => {
    await expect(settle(getComparison("demo", "r1", "r2"))).resolves.toBeNull();
  });
});

describe("getFileDiff", () => {
  it("answers the hunks of a file", async () => {
    const diff = await settle(
      getFileDiff("demo", "r7", "ws-7", "src/routes/login.tsx"),
    );
    expect(diff.hunks.length).toBeGreaterThan(0);
  });

  it("rejects a path the comparison does not list", async () => {
    const missing = getFileDiff("demo", "r7", "ws-7", "nope.ts");
    const check = expect(missing).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "path_not_in_comparison"),
    );
    await vi.runAllTimersAsync();
    await check;
  });

  it("rejects a file of a comparison it does not have", async () => {
    const missing = getFileDiff("demo", "r1", "r2", "a.ts");
    const check = expect(missing).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "path_not_in_comparison"),
    );
    await vi.runAllTimersAsync();
    await check;
  });
});

describe("diff queries", () => {
  it("key a comparison and a file under the project", async () => {
    const client = new QueryClient();
    const comparison = comparisonQuery("demo", "r7", "r8");
    const file = fileDiffQuery("demo", "r7", "r8", "CHANGELOG.md");
    expect(comparison.queryKey).toEqual(keys.comparison("demo", "r7", "r8"));
    expect(comparisonQuery("demo", undefined, "ws-7").queryKey).toEqual([
      "demo",
      "comparison",
      null,
      "ws-7",
    ]);
    expect(fileDiffQuery("demo", undefined, "ws-7", "a.ts").queryKey).toEqual([
      "demo",
      "file-diff",
      null,
      "ws-7",
      "a.ts",
    ]);
    expect(file.queryKey).toEqual([
      "demo",
      "file-diff",
      "r7",
      "r8",
      "CHANGELOG.md",
    ]);
    const loaded = settle(
      Promise.all([client.query(comparison), client.query(file)]),
    );
    const [c, f] = await loaded;
    expect(c?.files).toHaveLength(2);
    expect(f.path).toBe("CHANGELOG.md");
  });
});

describe("a production build", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it("has nothing to compare and reads no fixture", async () => {
    vi.stubEnv("DEV", false);
    vi.resetModules();
    const diff = await import("./diff");
    const errors = await import("./errors");
    await expect(diff.getComparison("demo", "r7", "ws-7")).resolves.toBeNull();
    await expect(
      diff.getFileDiff("demo", "r7", "ws-7", "a.ts"),
    ).rejects.toSatisfy((e: unknown) =>
      errors.isApiError(e, "path_not_in_comparison"),
    );
  });
});
