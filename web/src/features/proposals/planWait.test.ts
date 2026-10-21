import { describe, expect, it } from "vitest";
import { planWait, planWaitLabel, type ReviewPlanWait } from "./planWait";

const NOW = new Date(2026, 9, 21, 13, 10).getTime();
const DEADLINE = new Date(2026, 9, 21, 14, 5).getTime();

const plan = (fields: Partial<ReviewPlanWait> = {}): ReviewPlanWait => ({
  plan: "rp-2",
  deadline_at: DEADLINE,
  wait_ended: false,
  awaited: ["T-7", "T-8"],
  proposals: ["P-3", "P-4"],
  ...fields,
});

describe("planWait", () => {
  it("is null for a proposal in no plan", () => {
    expect(planWait(null, "P-3", NOW)).toBeNull();
  });

  it("names the awaited tasks and the end of the wait while tasks are awaited", () => {
    expect(planWait(plan(), "P-3", NOW)).toEqual({
      state: "waiting",
      label: "waiting for a batch",
      title: "Waiting for a batch",
      detail: "rp-2 waits for T-7, T-8 until 14:05",
    });
  });

  it("puts the date in front of a deadline on another day", () => {
    const tomorrow = new Date(2026, 9, 22, 0, 30).getTime();
    expect(planWait(plan({ deadline_at: tomorrow }), "P-3", NOW)?.detail).toBe(
      "rp-2 waits for T-7, T-8 until Oct 22, 00:30",
    );
  });

  it("waits for the reviewer once no task is awaited, naming the other proposals", () => {
    expect(planWait(plan({ awaited: [] }), "P-3", NOW)).toEqual({
      state: "ready",
      label: "batch ready",
      title: "Batch ready",
      detail: "rp-2 waits for the review lead to start one reviewer with P-4",
    });
    expect(
      planWait(plan({ wait_ended: true, proposals: ["P-3"] }), "P-3", NOW)
        ?.detail,
    ).toBe("rp-2 waits for the review lead to start one reviewer");
  });

  it("labels the pill by the same state", () => {
    expect(planWaitLabel(plan())).toBe("waiting for a batch");
    expect(planWaitLabel(plan({ wait_ended: true }))).toBe("batch ready");
  });
});
