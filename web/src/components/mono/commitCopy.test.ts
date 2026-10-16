import { describe, expect, it, vi } from "vitest";
import { copyOutcome } from "./commitCopy";

describe("copyOutcome", () => {
  it("copies the text and reports it", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    expect(await copyOutcome("abc", { writeText })).toBe("copied");
    expect(writeText).toHaveBeenCalledWith("abc");
  });

  it("reports a refused write", async () => {
    const writeText = vi.fn().mockRejectedValue(new Error("denied"));
    expect(await copyOutcome("abc", { writeText })).toBe("failed");
  });

  it("reports a browser without a clipboard", async () => {
    expect(await copyOutcome("abc", undefined)).toBe("failed");
  });
});
