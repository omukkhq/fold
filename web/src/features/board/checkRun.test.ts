import { describe, expect, it } from "vitest";
import type { LiveCheckRun } from "@/api/board";
import { checkRunText, currentCheckRun } from "./checkRun";

function run(over: Partial<LiveCheckRun> = {}): LiveCheckRun {
  return {
    run: "cr-4",
    purpose: "check",
    proposal: null,
    started_at: 1_000,
    ...over,
  };
}

describe("currentCheckRun", () => {
  it("takes the live frame's answer over the payload's, also when no run goes", () => {
    const payload = run();
    const streamed = run({ run: "cr-5" });
    expect(currentCheckRun(payload, { check_run: streamed })).toBe(streamed);
    expect(currentCheckRun(payload, { check_run: null })).toBeNull();
  });

  it("falls back to the payload without a live frame", () => {
    const payload = run();
    expect(currentCheckRun(payload, undefined)).toBe(payload);
    expect(currentCheckRun(payload, null)).toBe(payload);
    expect(currentCheckRun(null, undefined)).toBeNull();
  });
});

describe("checkRunText", () => {
  const proposal = { id: "P-3", version: 2 };

  it("names the version a proposal command waits to make", () => {
    expect(checkRunText(run({ purpose: "create_proposal" }), null)).toBe(
      "Running checks for v1",
    );
    expect(
      checkRunText(
        run({ purpose: "update_proposal", proposal: "P-3" }),
        proposal,
      ),
    ).toBe("Running checks for v3");
  });

  it("names no version for a worker's own run or a proposal it does not know", () => {
    expect(checkRunText(run(), proposal)).toBe("Running checks");
    expect(
      checkRunText(run({ purpose: "update_proposal", proposal: "P-3" }), null),
    ).toBe("Running checks");
    expect(
      checkRunText(
        run({ purpose: "update_proposal", proposal: "P-9" }),
        proposal,
      ),
    ).toBe("Running checks");
  });
});
