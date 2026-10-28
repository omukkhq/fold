import { describe, expect, it, vi } from "vitest";
import type { FileDiff } from "@/api/diff";
import { add, comparedFile, ctx, hunk } from "@/test/diff";
import { fileDiffView, fileTextView, needsFileText } from "./fileDiff";

const DIFF: FileDiff = {
  source: "recorded",
  path: "src/a.ts",
  previous_path: null,
  change: "added",
  binary: false,
  too_large: false,
  old_hash: null,
  new_hash: "h1",
  hunks: [],
};

describe("a file's hunks as the viewer draws them", () => {
  it("shows the hunks once they arrive and waits until then", () => {
    const refetch = vi.fn();
    expect(fileDiffView({ isError: false, data: DIFF, refetch })).toEqual({
      status: "ready",
      diff: DIFF,
    });
    expect(fileDiffView({ isError: false, data: undefined, refetch })).toEqual({
      status: "pending",
    });
  });

  it("offers a retry that reads the file again after a failed read", () => {
    const refetch = vi.fn();
    const view = fileDiffView({ isError: true, data: DIFF, refetch });
    expect(view.status).toBe("error");
    if (view.status === "error") view.retry();
    expect(refetch).toHaveBeenCalledOnce();
  });
});

describe("a file's new side as the viewer reads it", () => {
  const FILE = { hash: "h1", binary: false, too_large: false, text: "a\n" };

  it("shows the file once it arrives, waits until then and retries a failed read", () => {
    const refetch = vi.fn();
    expect(fileTextView({ isError: false, data: FILE, refetch })).toEqual({
      status: "ready",
      file: FILE,
    });
    expect(fileTextView({ isError: false, data: undefined, refetch })).toEqual({
      status: "pending",
    });
    const failed = fileTextView({ isError: true, data: undefined, refetch });
    if (failed.status === "error") failed.retry();
    expect(refetch).toHaveBeenCalledOnce();
  });

  it("is read only for a text diff with hunks of a file on both sides", () => {
    const modified = comparedFile("src/a.ts", "modified");
    const hunks = [hunk(3, 3, [ctx("a"), add("b")])];
    const ready = (extra: Partial<FileDiff> = {}) =>
      ({
        status: "ready",
        diff: { ...DIFF, change: "modified", old_hash: "h0", hunks, ...extra },
      }) as const;
    expect(needsFileText(modified, ready())).toBe(true);
    expect(needsFileText(null, ready())).toBe(false);
    expect(needsFileText(modified, { status: "pending" })).toBe(false);
    expect(needsFileText(modified, ready({ binary: true }))).toBe(false);
    expect(needsFileText(modified, ready({ too_large: true }))).toBe(false);
    expect(needsFileText(modified, ready({ hunks: [] }))).toBe(false);
    expect(needsFileText(modified, ready({ old_hash: null }))).toBe(false);
    expect(needsFileText(modified, ready({ new_hash: null }))).toBe(false);
    expect(
      needsFileText(
        comparedFile("logo.png", "modified", { binary: true }),
        ready(),
      ),
    ).toBe(false);
  });
});
