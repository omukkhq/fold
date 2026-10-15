import { describe, expect, it } from "vitest";
import type { ProposalSummary } from "@/api/proposals";
import {
  humanGates,
  proposalDetail,
  proposalSummary,
  review,
} from "@/test/proposals";
import { canComment, integrateBlock, visibleActions } from "./actions";

function proposal(fields: Partial<ProposalSummary> = {}) {
  return proposalDetail(
    proposalSummary("P-4", { current_version: 2, ...fields }),
  );
}

describe("visibleActions", () => {
  it("shows Approve, Request changes and Integrate to a named human on an open proposal", () => {
    expect(visibleActions(proposal(), "tanim")).toEqual([
      "approve",
      "request_changes",
      "integrate",
    ]);
  });

  it("shows Integrate after a request for changes on the current version, because the integrating human approves", () => {
    const p = proposalDetail(proposalSummary("P-3", { turn: "with_author" }), {
      reviews: [review(1, "review/security", "request_changes")],
    });
    expect(visibleActions(p, "tanim")).toContain("integrate");
  });

  it("shows nothing to a human with no name, and nothing on a proposal that is not open", () => {
    expect(visibleActions(proposal(), null)).toEqual([]);
    expect(visibleActions(proposal({ state: "integrated" }), "tanim")).toEqual(
      [],
    );
    expect(visibleActions(proposal({ state: "closed" }), "tanim")).toEqual([]);
  });

  it("never offers a reject", () => {
    expect(visibleActions(proposal(), "tanim")).not.toContain("reject");
  });
});

describe("canComment", () => {
  it("opens the composer only on the current version of an open proposal for a named human", () => {
    expect(canComment(proposal(), "tanim", 2)).toBe(true);
    expect(canComment(proposal(), "tanim", 1)).toBe(false);
    expect(canComment(proposal(), "tanim", null)).toBe(false);
    expect(canComment(proposal(), null, 2)).toBe(false);
    expect(canComment(proposal({ state: "closed" }), "tanim", 2)).toBe(false);
  });
});

describe("integrateBlock", () => {
  it("is null when the daemon reports no unmet gate for the human", () => {
    expect(integrateBlock(proposal(), "r8")).toBeNull();
  });

  it("names the human the mark waits for when the viewer is not that human", () => {
    const p = proposalDetail(
      proposalSummary("P-4", {
        current_version: 2,
        human_review: { reviewer: "tanim", reason: "", set_by: "backend" },
      }),
      {
        gates: humanGates({
          human_review: { status: "unmet", code: "human_approval_missing" },
        }),
      },
    );
    expect(integrateBlock(p, "r8")).toBe(
      "Integrate waits for tanim to approve v2.",
    );
    const anyHuman = {
      ...p,
      summary: { ...p.summary, human_review: null },
    };
    expect(integrateBlock(anyHuman, "r8")).toBe(
      "Integrate waits for a human to approve v2.",
    );
  });

  it("gives the first unmet gate's sentence for any other gate", () => {
    const p = proposalDetail(proposalSummary("P-7"), {
      gates: humanGates({
        prerequisites: { status: "unmet", code: "prerequisites_not_done" },
      }),
      prerequisites: [
        { task: "T-1", title: "T-1", description: "", status: "in_progress" },
      ],
    });
    expect(integrateBlock(p, "r8")).toBe(
      "Prerequisites not Done: T-1, so you cannot integrate yet.",
    );
  });

  it("ignores the merge gate, which only the integration decides", () => {
    const p = proposalDetail(proposalSummary("P-7"), {
      gates: humanGates({ merge: { status: "checked_at_integration" } }),
    });
    expect(integrateBlock(p, null)).toBeNull();
  });
});
