import { describe, expect, it } from "vitest";
import { comparedFile } from "@/test/diff";
import {
  changedFolders,
  changeTree,
  filesWord,
  folderPaths,
  foldersOf,
  matchesFilter,
  pathParts,
  toggleFolder,
  treeFiles,
  treeRows,
  type ChangeNode,
  type TreeRow,
} from "./folderTree";

const files = [
  comparedFile("backend/app/main.py", "modified", {
    added_lines: 4,
    removed_lines: 1,
  }),
  comparedFile("backend/app/core/files.py", "added", {
    added_lines: 154,
    removed_lines: 0,
  }),
  comparedFile("backend/alembic/versions/0001_files.py", "added", {
    added_lines: 100,
    removed_lines: 0,
  }),
  comparedFile("backend/app/core/policy.py", "modified", {
    added_lines: 70,
    removed_lines: 2,
  }),
  comparedFile("docs/files.md", "deleted", {
    added_lines: 0,
    removed_lines: 15,
  }),
  comparedFile("README.md", "modified", { added_lines: 3, removed_lines: 3 }),
];

function names(rows: readonly TreeRow[]): string[] {
  return rows.map(
    (row) =>
      `${"  ".repeat(row.depth)}${row.node.name}${row.node.kind === "folder" ? "/" : ""}`,
  );
}

function folder(nodes: readonly ChangeNode[], name: string): ChangeNode {
  const found = nodes.find((n) => n.name === name);
  if (!found) throw new Error(`no ${name}`);
  return found;
}

describe("changeTree", () => {
  it("groups files in folders before files, and one row holds a chain of single-child folders", () => {
    const tree = changeTree(files);
    expect(names(treeRows(tree, new Set()))).toEqual([
      "backend/",
      "  alembic/versions/",
      "    0001_files.py",
      "  app/",
      "    core/",
      "      files.py",
      "      policy.py",
      "    main.py",
      "docs/",
      "  files.md",
      "README.md",
    ]);
  });

  it("totals the files and lines below each folder", () => {
    const tree = changeTree(files);
    const backend = folder(tree, "backend");
    expect(backend.kind === "folder" && backend.totals).toEqual({
      files: 4,
      added: 328,
      removed: 3,
    });
    const docs = folder(tree, "docs");
    expect(docs.kind === "folder" && docs.totals).toEqual({
      files: 1,
      added: 0,
      removed: 15,
    });
  });

  it("pairs a delete and an add of the same content as one rename", () => {
    const tree = changeTree([
      comparedFile("a/old.ts", "deleted", { old_hash: "h" }),
      comparedFile("a/new.ts", "added", { new_hash: "h" }),
    ]);
    const files = treeFiles(tree);
    expect(files.map((f) => [f.path, f.change, f.previous_path])).toEqual([
      ["a/new.ts", "renamed", "a/old.ts"],
    ]);
  });
});

describe("treeRows", () => {
  it("hides the entries of a closed folder", () => {
    const tree = changeTree(files);
    const rows = treeRows(tree, new Set(["backend/app", "docs"]));
    expect(names(rows)).toEqual([
      "backend/",
      "  alembic/versions/",
      "    0001_files.py",
      "  app/",
      "docs/",
      "README.md",
    ]);
    expect(rows.find((r) => r.node.path === "backend/app")?.open).toBe(false);
    expect(rows.find((r) => r.node.path === "backend")?.open).toBe(true);
    expect(rows.find((r) => r.node.path === "README.md")?.open).toBe(false);
  });

  it("shows only the matching files and their folders, open even when closed", () => {
    const tree = changeTree(files);
    const rows = treeRows(tree, new Set(folderPaths(tree)), "FILES");
    expect(names(rows)).toEqual([
      "backend/",
      "  alembic/versions/",
      "    0001_files.py",
      "  app/",
      "    core/",
      "      files.py",
      "docs/",
      "  files.md",
    ]);
  });

  it("needs every word of the filter in the path", () => {
    const tree = changeTree(files);
    expect(names(treeRows(tree, new Set(), " core  pol "))).toEqual([
      "backend/",
      "  app/",
      "    core/",
      "      policy.py",
    ]);
    expect(treeRows(tree, new Set(), "nothing")).toEqual([]);
  });
});

describe("matchesFilter", () => {
  it("matches every path with an empty filter", () => {
    expect(matchesFilter("a/b.ts", "   ")).toBe(true);
    expect(matchesFilter("a/B.ts", "b.T")).toBe(true);
    expect(matchesFilter("a/b.ts", "c")).toBe(false);
  });
});

describe("folderPaths and foldersOf", () => {
  it("lists every folder, and the folders that hold one file", () => {
    const tree = changeTree(files);
    expect(folderPaths(tree)).toEqual([
      "backend",
      "backend/alembic/versions",
      "backend/app",
      "backend/app/core",
      "docs",
    ]);
    expect(foldersOf(tree, "backend/app/core/policy.py")).toEqual([
      "backend",
      "backend/app",
      "backend/app/core",
    ]);
    expect(foldersOf(tree, "README.md")).toEqual([]);
    expect(foldersOf(tree, "missing/x.ts")).toEqual([]);
  });
});

describe("changedFolders", () => {
  it("lists the folders of each top folder, the files directly in it, and the files at the root", () => {
    expect(changedFolders(changeTree(files))).toEqual([
      {
        path: "backend/alembic/versions",
        totals: { files: 1, added: 100, removed: 0 },
        first: "backend/alembic/versions/0001_files.py",
      },
      {
        path: "backend/app",
        totals: { files: 3, added: 228, removed: 3 },
        first: "backend/app/core/files.py",
      },
      {
        path: "docs",
        totals: { files: 1, added: 0, removed: 15 },
        first: "docs/files.md",
      },
      {
        path: "",
        totals: { files: 1, added: 3, removed: 3 },
        first: "README.md",
      },
    ]);
  });

  it("gives the files directly in a top folder their own line", () => {
    const tree = changeTree([
      comparedFile("web/index.html", "modified"),
      comparedFile("web/src/a.ts", "modified"),
    ]);
    expect(changedFolders(tree).map((f) => [f.path, f.totals.files])).toEqual([
      ["web/src", 1],
      ["web", 1],
    ]);
  });

  it("is empty for no files", () => {
    expect(changedFolders(changeTree([]))).toEqual([]);
  });
});

describe("toggleFolder", () => {
  it("opens a closed folder and closes an open one", () => {
    expect(toggleFolder(new Set(["a"]), "a")).toEqual(new Set());
    expect(toggleFolder(new Set(), "a")).toEqual(new Set(["a"]));
  });
});

describe("words", () => {
  it("counts files and splits a path", () => {
    expect(filesWord(1)).toBe("1 file");
    expect(filesWord(3)).toBe("3 files");
    expect(pathParts("a/b/c.ts")).toEqual({ folder: "a/b/", name: "c.ts" });
    expect(pathParts("c.ts")).toEqual({ folder: "", name: "c.ts" });
  });
});
