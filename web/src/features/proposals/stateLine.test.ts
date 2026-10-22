import { describe, expect, it } from "vitest";
import {
  checkRun,
  proposalDetail,
  proposalSummary,
  review,
} from "@/test/proposals";
import { approvalLine, checksWord, runsCount } from "./stateLine";

const p = proposalDetail(proposalSummary("P-4", { current_version: 3 }), {
  reviews: [
    review(1, "review/quality", "request_changes"),
    review(2, "review/quality", "comment"),
    review(3, "review/lead", "approve"),
    review(3, "human:rafeed", "approve"),
  ],
});

describe("approvalLine", () => {
  it("names who approved the version", () => {
    expect(approvalLine(p, 3)).toEqual({
      tone: "approved",
      segments: [
        "Approved on v3 by ",
        { actor: "review/lead" },
        " and ",
        { actor: "human:rafeed" },
      ],
    });
  });

  it("says the latest verdict, or that none came", () => {
    expect(approvalLine(p, 1)).toEqual({
      tone: "changes",
      segments: ["Changes requested on v1"],
    });
    expect(approvalLine(p, 2)).toEqual({
      tone: "neutral",
      segments: ["No verdict on v2 yet"],
    });
  });
});

describe("checksWord", () => {
  it("says what the newest run did", () => {
    expect(checksWord(null)).toBeNull();
    expect(checksWord([])).toBe("no checks ran");
    expect(
      checksWord([
        checkRun(1, { created_at: 1, state: "failed" }),
        checkRun(2, { created_at: 2 }),
      ]),
    ).toBe("checks passed");
    expect(checksWord([checkRun(3, { state: "running" })])).toBe(
      "checks running",
    );
  });

  it("counts runs", () => {
    expect(runsCount(null)).toBeNull();
    expect(runsCount(1)).toBe("1 run");
    expect(runsCount(2)).toBe("2 runs");
  });
});
