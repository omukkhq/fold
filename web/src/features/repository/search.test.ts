import { describe, expect, it } from "vitest";
import {
  folderAt,
  parentDirs,
  parentOf,
  parseRepositorySearch,
} from "./search";

describe("parseRepositorySearch", () => {
  it("keeps a revision id", () => {
    expect(parseRepositorySearch({ revision: "r17" })).toEqual({
      revision: "r17",
    });
  });

  it("keeps the genesis r0", () => {
    expect(parseRepositorySearch({ revision: "r0" })).toEqual({
      revision: "r0",
    });
  });

  it("ignores anything else", () => {
    for (const revision of [
      "T-4",
      "r00",
      "r04",
      "r1 ",
      "main",
      "",
      17,
      null,
      ["r1"],
    ]) {
      expect(parseRepositorySearch({ revision })).toEqual({});
    }
    expect(parseRepositorySearch({})).toEqual({});
  });
});

describe("path", () => {
  it("keeps a clean relative path beside the revision", () => {
    expect(
      parseRepositorySearch({ revision: "r2", path: "web/src/a.ts" }),
    ).toEqual({
      revision: "r2",
      path: "web/src/a.ts",
    });
  });

  it("ignores a path that climbs, starts at the root or is not text", () => {
    for (const path of ["../x", "/etc/passwd", "a//b", "a/./b", "", 3]) {
      expect(parseRepositorySearch({ path })).toEqual({});
    }
  });

  it("lists the directories above a path", () => {
    expect(parentDirs("web/src/a.ts")).toEqual(["web", "web/src"]);
    expect(parentDirs("README.md")).toEqual([]);
  });
});

describe("parentOf", () => {
  it("names the directory that holds a path", () => {
    expect(parentOf("web/src/board.ts")).toBe("web/src");
    expect(parentOf("web")).toBe("");
  });
});

describe("folderAt", () => {
  const entries = [
    { path: "web/src", kind: "directory" },
    { path: "web/app.ts", kind: "file" },
  ];

  it("tells a folder from a file and an unlisted path", () => {
    expect(folderAt(entries, "web/src")).toBe(true);
    expect(folderAt(entries, "web/app.ts")).toBe(false);
    expect(folderAt(entries, "web/gone")).toBeNull();
  });
});
