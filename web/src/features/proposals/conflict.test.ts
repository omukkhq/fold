import { describe, expect, it } from "vitest";
import type { ConflictKind } from "@/api/proposals";
import { detail } from "../../../fixtures/proposals";
import {
  conflictKindText,
  entryText,
  fileSides,
  nextStepText,
  reportTrees,
} from "./conflict";

const report = detail("demo", "P-6")?.conflict;

describe("conflict report text", () => {
  it("names every kind of conflict", () => {
    const kinds: ConflictKind[] = [
      "both_modified_text",
      "both_modified_binary",
      "modify_delete",
      "add_add",
      "type_change",
    ];
    expect(new Set(kinds.map(conflictKindText)).size).toBe(5);
    expect(conflictKindText("both_modified_text")).toMatch(/^Text conflict/);
  });

  it("shows each side of a file with its entry, or nothing", () => {
    if (!report) throw new Error("no report");
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
    if (!report) throw new Error("no report");
    expect(reportTrees(report, "P-6 v1")).toEqual([
      { label: "base r7", entry: `tree ${report.trees.base.slice(0, 7)}` },
      { label: "main r8", entry: `tree ${report.trees.main.slice(0, 7)}` },
      { label: "P-6 v1", entry: `tree ${report.trees.version.slice(0, 7)}` },
    ]);
    expect(nextStepText("frontend/charts", 1)).toBe(
      "Next: frontend/charts runs sync_workspace, resolves, and sends v2.",
    );
  });
});
