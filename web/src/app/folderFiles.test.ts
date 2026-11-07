import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import type { SharedFile } from "@/api/folderDiff";
import { comparedFile } from "@/test/diff";
import {
  flaggedFacts,
  flaggedFiles,
  folderFailure,
  sharedFileFacts,
} from "./folderFiles";

describe("the files of a worker's change", () => {
  it("keeps the named order and marks a file the folder holds as the revision's again", () => {
    const plan = comparedFile("plan.md", "modified");
    expect(flaggedFiles([plan], ["junk.txt", "plan.md"])).toEqual([
      { path: "junk.txt", file: null },
      { path: "plan.md", file: plan },
    ]);
  });

  it("names each file's change and line counts", () => {
    const facts = (file: ReturnType<typeof comparedFile> | null) =>
      flaggedFacts({ path: "a", file });
    expect(facts(null)).toBe("holds the revision's version again");
    expect(
      facts(
        comparedFile("a", "modified", { added_lines: 0, removed_lines: 3 }),
      ),
    ).toBe("−3");
    expect(
      facts(comparedFile("a", "added", { added_lines: 2, removed_lines: 0 })),
    ).toBe("new file · +2");
    expect(
      facts(
        comparedFile("a", "modified", {
          binary: true,
          added_lines: 0,
          removed_lines: 0,
        }),
      ),
    ).toBe("binary");
    expect(
      facts(
        comparedFile("a", "modified", { added_lines: 0, removed_lines: 0 }),
      ),
    ).toBe("changed");
    expect(
      facts(comparedFile("a", "deleted", { added_lines: 0, removed_lines: 4 })),
    ).toBe("deleted · −4");
  });
});

describe("a changed shared file", () => {
  const now = new Date(2026, 10, 7, 15, 0).getTime();
  const at = new Date(2026, 10, 7, 14, 12).getTime();
  const file = (fields: Partial<SharedFile>): SharedFile => ({
    path: "design/home.pen",
    kind: "file",
    size: 2048,
    modified: at,
    ...fields,
  });

  it("names its size and change time, a folder by kind and a link without following it", () => {
    expect(sharedFileFacts(file({}), now)).toBe("2.0 KB · changed 14:12");
    expect(sharedFileFacts(file({ kind: "folder", size: null }), now)).toBe(
      "folder · changed 14:12",
    );
    expect(sharedFileFacts(file({ kind: "symlink", size: 13 }), now)).toBe(
      "symbolic link, 13 B · changed 14:12",
    );
    expect(sharedFileFacts(file({ modified: null }), now)).toBe("2.0 KB");
    expect(
      sharedFileFacts(
        file({ kind: "missing", size: null, modified: null }),
        now,
      ),
    ).toBe("gone from the project folder");
  });
});

describe("folderFailure", () => {
  it("words a failed read of the folder's diff by its code", () => {
    expect(folderFailure(new ApiError("workspace_io", "x"))).toBe(
      "fold could not read a file of the project folder.",
    );
    expect(folderFailure(new ApiError("path_not_in_comparison", "x"))).toBe(
      "The file holds the revision's version again.",
    );
    expect(folderFailure(new ApiError("internal_error", "x"))).toBe(
      "The daemon failed. Try again.",
    );
    expect(folderFailure(new ApiError("auth_required", "x"))).toBe(
      "Sign in again, then try once more.",
    );
    expect(folderFailure(new ApiError("other", "x"))).toBe(
      "The daemon refused the request.",
    );
    expect(folderFailure(new Error("offline"))).toBe(
      "The daemon did not answer.",
    );
  });
});
