import { describe, expect, it } from "vitest";
import type { ConflictKind } from "@/api/proposals";
import { conflictReport } from "@/test/proposals";
import {
  conflictKindText,
  entryText,
  fileSides,
  hunkSides,
  nextStepText as nextStepTextSegments,
  reportTrees,
} from "./conflict";
import { segmentsText } from "@/people/segments";

const nextStepText = (...args: Parameters<typeof nextStepTextSegments>) =>
  segmentsText(nextStepTextSegments(...args));

const report = conflictReport("P-6", 1);

describe("conflict report text", () => {
  it("names every kind of conflict", () => {
    const kinds: ConflictKind[] = [
      "text",
      "both_modified_binary",
      "modify_delete",
      "add_add",
      "type_change",
    ];
    expect(new Set(kinds.map(conflictKindText)).size).toBe(5);
    expect(conflictKindText("text")).toMatch(/^Text conflict/);
  });

  it("shows each side of a file with its entry, or nothing", () => {
    const deleted = report.files.find((f) => f.kind === "modify_delete");
    if (!deleted) throw new Error("no modify_delete file");
    expect(fileSides(deleted, "P-6 v1", report)).toEqual([
      {
        label: "base r7",
        entry: `file ${deleted.base?.hash.slice(0, 7) ?? ""}`,
      },
      { label: "main r8", entry: "nothing" },
      {
        label: "P-6 v1",
        entry: `file ${deleted.version?.hash.slice(0, 7) ?? ""}`,
      },
    ]);
    expect(entryText({ kind: "symlink", hash: "abcdef0123" })).toBe(
      "symlink abcdef0",
    );
  });

  it("names the trees and the author's next step", () => {
    expect(reportTrees(report, "P-6 v1")).toEqual([
      { label: "base r7", entry: `tree ${report.trees.base.slice(0, 7)}` },
      { label: "main r8", entry: `tree ${report.trees.main.slice(0, 7)}` },
      { label: "P-6 v1", entry: `tree ${report.trees.version.slice(0, 7)}` },
    ]);
    expect(nextStepText("frontend/charts", 1)).toBe(
      "Next: @frontend/charts runs sync_workspace, resolves, and sends v2.",
    );
  });

  it("names each side of a text hunk by the lines it spans, without line terminators", () => {
    const hunk = report.files[0]?.hunks[0];
    if (!hunk) throw new Error("no hunk");
    expect(hunkSides(hunk, "P-6 v1")).toEqual([
      { label: "base · line 4", lines: ["const max = 1;"] },
      {
        label: "main · line 4",
        lines: ["const max = Math.max(...totals, 1);"],
      },
      { label: "P-6 v1 · after line 3", lines: [] },
    ]);
    expect(
      hunkSides(
        {
          base: { start: 2, count: 2, lines: ["a\r\n", "b"] },
          main: { start: 2, count: 1, lines: ["c\n"] },
          version: { start: 2, count: 1, lines: ["d\n"] },
        },
        "v",
      )[0],
    ).toEqual({ label: "base · lines 2–3", lines: ["a", "b"] });
  });
});
