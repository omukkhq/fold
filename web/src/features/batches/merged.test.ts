import { describe, expect, it } from "vitest";
import { comparedFile } from "@/test/diff";
import { batchDiff } from "@/test/workers";
import {
  batchHref,
  batchTab,
  batchTabSearch,
  conflictNote,
  isBatchTab,
  mergedEmpty,
  mergedLine,
  mergedViewedKey,
  parseBatchSearch,
} from "./merged";

const FILE = comparedFile("src/a.ts", "added");

describe("the batch screen's address", () => {
  it("keeps a known tab and a file and drops the rest", () => {
    expect(parseBatchSearch({ tab: "changes", path: " src/a.ts " })).toEqual({
      tab: "changes",
      path: "src/a.ts",
    });
    expect(parseBatchSearch({ tab: "checks", path: "", other: 1 })).toEqual({});
    expect(parseBatchSearch({ path: 3 })).toEqual({});
    expect(isBatchTab("members")).toBe(true);
    expect(isBatchTab("overview")).toBe(false);
  });

  it("opens Merged changes for a file and Members otherwise", () => {
    expect(batchTab({})).toBe("members");
    expect(batchTab({ path: "src/a.ts" })).toBe("changes");
    expect(batchTab({ tab: "members", path: "src/a.ts" })).toBe("members");
    expect(batchTab({ tab: "changes" })).toBe("changes");
  });

  it("keeps the file only on the Merged changes tab", () => {
    expect(batchTabSearch({ tab: "changes", path: "a" }, "members")).toEqual(
      {},
    );
    expect(batchTabSearch({ path: "a" }, "changes")).toEqual({
      tab: "changes",
      path: "a",
    });
    expect(batchTabSearch({}, "changes")).toEqual({ tab: "changes" });
  });

  it("writes the tab and the file into the link", () => {
    expect(batchHref("demo", "rb-3", {})).toBe("/p/demo/batches/rb-3");
    expect(batchHref("demo", "rb-3", { tab: "changes", path: "src/a b" })).toBe(
      "/p/demo/batches/rb-3?tab=changes&path=src%2Fa+b",
    );
  });
});

describe("the merged change", () => {
  it("names main and the members merged in order", () => {
    expect(mergedLine(batchDiff())).toBe(
      "main r17 + P-3 v2 + P-4 v1, merged in order",
    );
    expect(mergedLine(batchDiff({ members: [] }))).toBe(
      "main r17, before any member",
    );
  });

  it("names the revisions an integrated batch published", () => {
    expect(
      mergedLine(batchDiff({ basis: "integrated", base: "r17", head: "r19" })),
    ).toBe("main r17 → r19: P-3 v2 + P-4 v1, integrated together");
  });

  it("has no sides when it compares nothing", () => {
    for (const basis of ["no_members", "closed", "not_together"] as const) {
      expect(
        mergedLine(batchDiff({ basis, base: null, members: [] })),
      ).toBeNull();
    }
  });

  it("says why a change without files is empty", () => {
    expect(mergedEmpty(batchDiff({ basis: "no_members" }))?.title).toBe(
      "No members to merge",
    );
    expect(mergedEmpty(batchDiff({ basis: "closed" }))?.title).toBe(
      "No merged change",
    );
    expect(mergedEmpty(batchDiff({ basis: "not_together" }))?.text).toMatch(
      /Each member's page shows its own change/,
    );
    expect(mergedEmpty(batchDiff())?.title).toBe("No changes");
    expect(mergedEmpty(batchDiff({ basis: "integrated" }))?.title).toBe(
      "No changes",
    );
    expect(mergedEmpty(batchDiff({ files: [FILE] }))).toBeNull();
    expect(
      mergedEmpty(
        batchDiff({
          members: [],
          conflict: { proposal: "P-3", version: 2, with: null, paths: ["a"] },
        }),
      ),
    ).toEqual({
      title: "No member merges",
      text: "P-3 is the first member, so no change merges before it.",
    });
  });

  it("names the member that conflicts and what it conflicts with", () => {
    expect(conflictNote(batchDiff())).toBeNull();
    expect(
      conflictNote(
        batchDiff({
          members: [{ proposal: "P-3", version: 2 }],
          conflict: {
            proposal: "P-4",
            version: 1,
            with: "P-3",
            paths: ["README.md"],
          },
        }),
      ),
    ).toEqual({
      title: "P-4 v1 conflicts with P-3",
      text: "The change shows only P-3 v2, the members before it. Conflicted file: README.md.",
    });
    expect(
      conflictNote(
        batchDiff({
          members: [],
          conflict: {
            proposal: "P-3",
            version: 2,
            with: null,
            paths: ["a.ts", "b.ts"],
          },
        }),
      ),
    ).toEqual({
      title: "P-3 v2 conflicts with main",
      text: "No member merges before it. Conflicted files: a.ts, b.ts.",
    });
  });

  it("keys the viewed marks by the sides it compares", () => {
    expect(mergedViewedKey("demo", "rb-3", batchDiff())).toBe(
      "fold.viewed.demo.rb-3.r17+P-3v2+P-4v1",
    );
    expect(
      mergedViewedKey(
        "demo",
        "rb-3",
        batchDiff({ basis: "integrated", base: "r17", head: "r19" }),
      ),
    ).toBe("fold.viewed.demo.rb-3.r17..r19");
    expect(
      mergedViewedKey(
        "demo",
        "rb-3",
        batchDiff({ basis: "integrated", base: null, head: null }),
      ),
    ).toBe("fold.viewed.demo.rb-3...");
    expect(
      mergedViewedKey(
        "demo",
        "rb-3",
        batchDiff({ basis: "closed", base: null, members: [] }),
      ),
    ).toBe("fold.viewed.demo.rb-3.+");
  });
});
