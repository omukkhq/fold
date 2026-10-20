import { describe, expect, it } from "vitest";
import type { ProposalSummary } from "@/api/proposals";
import {
  humanGates,
  proposalDetail,
  proposalSummary,
  proposalVersion,
  review,
} from "@/test/proposals";
import {
  actedVersion,
  AGENT_APPROVES_FIRST,
  APPROVE_BODY,
  approveBlock,
  CHECKS_FAILED,
  humanVerdict,
  integrateBlock,
  showsApproveLine,
  visibleActions,
} from "./actions";
import { resolveView } from "./view";

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

  it("adds Send for review again on a human edit whose kept approvals count", () => {
    const edited = proposalDetail(
      proposalSummary("P-4", {
        current_version: 2,
        human_review: { reason: "auth", reviewer: null, set_by: "planner" },
      }),
      {
        versions: [
          proposalVersion(1),
          proposalVersion(2, { created_by: "human:tanim", human_edit: true }),
        ],
      },
    );
    expect(visibleActions(edited, "tanim")).toEqual([
      "approve",
      "request_changes",
      "integrate",
      "review_again",
    ]);
  });

  it("never offers a reject", () => {
    expect(visibleActions(proposal(), "tanim")).not.toContain("reject");
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
        agent_approved: true,
      },
    );
    expect(integrateBlock(p, "r8")).toBe(
      "Integrate waits for @tanim to approve v2.",
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

describe("actedVersion and humanVerdict", () => {
  const twoVersions = () => proposal({ current_version: 2 });

  it("act on the version on screen, not the current version, so a version that arrived meanwhile is refused", () => {
    const shownV1 = resolveView(twoVersions(), { version: 1 });
    expect(actedVersion(shownV1)).toBe(1);
    expect(humanVerdict(shownV1, "approve", APPROVE_BODY)).toEqual({
      version: 1,
      verdict: "approve",
      body: APPROVE_BODY,
    });
    expect(humanVerdict(shownV1, "request_changes", "Add a test.")).toEqual({
      version: 1,
      verdict: "request_changes",
      body: "Add a test.",
    });
  });

  it("act on the current version when it is the one on screen", () => {
    const shown = resolveView(twoVersions(), {});
    expect(actedVersion(shown)).toBe(2);
    expect(humanVerdict(shown, "approve", APPROVE_BODY).version).toBe(2);
  });
});

describe("approveBlock", () => {
  const mark = { reviewer: null, reason: "billing", set_by: "planner" };

  it("blocks Approve and Integrate once the integration checks of the current version failed", () => {
    const failed = proposalDetail(
      proposalSummary("P-4", { checks_failed: true, turn: "with_author" }),
      { agent_approved: true },
    );
    expect(CHECKS_FAILED).toBe(
      "The integration checks failed; the author sends a new version.",
    );
    expect(approveBlock(failed)).toBe(CHECKS_FAILED);
    expect(integrateBlock(failed, "r8")).toBe(CHECKS_FAILED);
    const marked = proposalDetail(
      proposalSummary("P-4", { checks_failed: true, human_review: mark }),
      { agent_approved: false },
    );
    expect(approveBlock(marked)).toBe(CHECKS_FAILED);
  });

  it("blocks a human's Approve on a marked task until a reviewer agent approved the current version", () => {
    const waiting = proposalDetail(
      proposalSummary("P-4", { human_review: mark }),
      { agent_approved: false },
    );
    expect(approveBlock(waiting)).toBe(AGENT_APPROVES_FIRST);
    expect(AGENT_APPROVES_FIRST).toBe("A reviewer agent approves first.");
    const approved = proposalDetail(
      proposalSummary("P-4", { human_review: mark }),
      { agent_approved: true },
    );
    expect(approveBlock(approved)).toBeNull();
  });

  it("lets a human approve a task of the main agent's area first, since no reviewer agent reviews it", () => {
    const main = proposalDetail(
      proposalSummary("P-4", { human_review: mark }),
      { agent_approved: false, main_task: true },
    );
    expect(approveBlock(main)).toBeNull();
    expect(integrateBlock(main, "r8")).not.toBe(AGENT_APPROVES_FIRST);
  });

  it("blocks Integrate on a marked task with the same line, because integrating approves", () => {
    const waiting = proposalDetail(
      proposalSummary("P-4", { human_review: mark }),
      { agent_approved: false },
    );
    expect(integrateBlock(waiting, "r8")).toBe(AGENT_APPROVES_FIRST);
    const approved = proposalDetail(
      proposalSummary("P-4", { human_review: mark }),
      { agent_approved: true },
    );
    expect(integrateBlock(approved, "r8")).not.toBe(AGENT_APPROVES_FIRST);
  });

  it("never blocks Approve on a task without the mark", () => {
    expect(approveBlock(proposal())).toBeNull();
  });
});

describe("showsApproveLine", () => {
  it("shows the failed checks line once, under both buttons, while Integrate shows", () => {
    expect(showsApproveLine(CHECKS_FAILED, CHECKS_FAILED, true)).toBe(false);
    expect(showsApproveLine(CHECKS_FAILED, CHECKS_FAILED, false)).toBe(true);
    expect(showsApproveLine(CHECKS_FAILED, null, true)).toBe(true);
  });

  it("keeps every other line under Approve", () => {
    expect(
      showsApproveLine(AGENT_APPROVES_FIRST, AGENT_APPROVES_FIRST, true),
    ).toBe(true);
    expect(showsApproveLine(null, CHECKS_FAILED, true)).toBe(false);
  });
});
