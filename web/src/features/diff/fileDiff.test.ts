import { describe, expect, it, vi } from "vitest";
import type { FileDiff } from "@/api/diff";
import { fileDiffView } from "./fileDiff";

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
