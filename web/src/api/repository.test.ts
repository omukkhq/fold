import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { isApiError } from "./errors";
import {
  directoryEntries,
  getFile,
  getTree,
  listRevisions,
  type FileNode,
} from "./repository";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

async function settled<T>(promise: Promise<T>): Promise<T> {
  const done = promise.then(
    (value) => ({ value }),
    (error: unknown) => ({ error }),
  );
  await vi.runAllTimersAsync();
  const result = await done;
  if ("error" in result) {
    throw result.error;
  }
  return result.value;
}

describe("directoryEntries", () => {
  const nodes: Record<string, FileNode> = {
    "b.txt": { kind: "text", content: "bb" },
    "a/x.ts": { kind: "text", content: "x" },
    "a/deep/y.ts": { kind: "text", content: "y" },
    link: { kind: "symlink", target: "a/x.ts" },
    "logo.png": { kind: "binary", size: 9, hash: "h" },
  };

  it("lists directories first, then by name, with link targets and sizes", () => {
    expect(directoryEntries(nodes, "")).toEqual([
      { name: "a", path: "a", kind: "directory" },
      { name: "b.txt", path: "b.txt", kind: "file", size: 2 },
      { name: "link", path: "link", kind: "symlink", target: "a/x.ts" },
      { name: "logo.png", path: "logo.png", kind: "file", size: 9 },
    ]);
  });

  it("lists one directory without its descendants", () => {
    expect(directoryEntries(nodes, "a")).toEqual([
      { name: "deep", path: "a/deep", kind: "directory" },
      { name: "x.ts", path: "a/x.ts", kind: "file", size: 1 },
    ]);
  });

  it("does not take a sibling with the same prefix for a child", () => {
    expect(
      directoryEntries({ "ab/x": { kind: "text", content: "" } }, "a"),
    ).toEqual([]);
  });
});

describe("repository fixture", () => {
  it("lists revisions newest first", async () => {
    const revisions = await settled(listRevisions("demo"));
    expect(revisions.map((r) => r.id)).toEqual(["r5", "r4", "r3", "r2", "r1"]);
  });

  it("has only r1 for a young project and nothing for an unknown one", async () => {
    expect((await settled(listRevisions("ledger"))).map((r) => r.id)).toEqual([
      "r1",
    ]);
    expect(await settled(listRevisions("nope"))).toEqual([]);
  });

  it("reads the tree of one revision", async () => {
    const r1 = await settled(getTree("demo", "r1", ""));
    const r5 = await settled(getTree("demo", "r5", ""));
    expect(r1.map((e) => e.name)).not.toContain("docs");
    expect(r5.find((e) => e.name === "docs")?.kind).toBe("directory");
    const docs = await settled(getTree("demo", "r5", "docs"));
    expect(docs[0]).toMatchObject({ kind: "symlink", target: "../README.md" });
  });

  it("rejects an unknown revision with its code", async () => {
    await expect(settled(getTree("demo", "r99", ""))).rejects.toSatisfy(
      (e: unknown) => isApiError(e, "revision_not_found"),
    );
  });

  it("opens a text file and a binary file without content", async () => {
    const text = await settled(getFile("demo", "r5", "README.md"));
    expect(text.kind).toBe("text");
    const binary = await settled(getFile("demo", "r5", "assets/logo.png"));
    expect(binary).toEqual({
      kind: "binary",
      path: "assets/logo.png",
      size: 18_432,
      hash: "b3c1d2e4f5a60718",
    });
    expect("content" in binary).toBe(false);
  });

  it("does not open a symlink or a missing path as a file", async () => {
    for (const path of ["docs/latest", "nope.txt"]) {
      await expect(settled(getFile("demo", "r5", path))).rejects.toSatisfy(
        (e: unknown) => isApiError(e, "not_found"),
      );
    }
  });
});

describe("a production build", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it("holds no repository data", async () => {
    vi.stubEnv("DEV", false);
    vi.resetModules();
    const repository = await import("./repository");
    await expect(repository.listRevisions("demo")).resolves.toEqual([]);
  });
});
