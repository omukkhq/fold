import { describe, expect, it } from "vitest";
import type { ProposalSummary } from "@/api/proposals";
import {
  checkRun,
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
  checkRunsLine,
  humanVerdict,
  integrateBlock,
  integrationRunsLine,
  ownApprovalLine,
  updateRunsLine,
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

  it("waits while an integration check run of the current version runs, and not for a proposal-stage run", () => {
    const running = (stage: "proposal" | "integration") =>
      proposalDetail(proposalSummary("P-46", { current_version: 5 }), {
        checks: [
          checkRun(163, {
            stage,
            state: "running",
            finished_at: null,
            version: stage === "integration" ? 5 : null,
          }),
          checkRun(152),
        ],
      });
    expect(integrateBlock(running("integration"), "r76")).toBe(
      "fold runs the integration checks of v5. Integrate when they end.",
    );
    expect(integrationRunsLine(5)).toBe(
      integrateBlock(running("integration"), "r76"),
    );
    expect(integrateBlock(running("proposal"), "r76")).toBeNull();
    const ended = proposalDetail(
      proposalSummary("P-46", { current_version: 5 }),
      { checks: [checkRun(163, { stage: "integration", state: "passed" })] },
    );
    expect(integrateBlock(ended, "r76")).toBeNull();
    const failed = {
      ...running("integration"),
      summary: proposalSummary("P-46", {
        current_version: 5,
        checks_failed: true,
      }),
    };
    expect(integrateBlock(failed, "r76")).toBe(CHECKS_FAILED);
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
    expect(approveBlock(failed, "tanim")).toBe(CHECKS_FAILED);
    expect(integrateBlock(failed, "r8")).toBe(CHECKS_FAILED);
    const marked = proposalDetail(
      proposalSummary("P-4", { checks_failed: true, human_review: mark }),
      { agent_approved: false },
    );
    expect(approveBlock(marked, "tanim")).toBe(CHECKS_FAILED);
  });

  it("blocks Approve and Integrate while an update runs the checks of the next version", () => {
    const updating = proposalDetail(
      proposalSummary("P-4", { current_version: 2, human_review: mark }),
      { agent_approved: false, update_run: "cr-7" },
    );
    expect(updateRunsLine(3)).toBe("v3 is running its checks.");
    expect(approveBlock(updating, "tanim")).toBe("v3 is running its checks.");
    expect(integrateBlock(updating, "r8")).toBe("v3 is running its checks.");
    const approved = proposalDetail(
      proposalSummary("P-4", { current_version: 2 }),
      {
        reviews: [review(2, "human:tanim", "approve")],
        update_run: "cr-7",
      },
    );
    expect(approveBlock(approved, "tanim")).toBe(updateRunsLine(3));
    const ended = { ...updating, update_run: null };
    expect(approveBlock(ended, "tanim")).toBe(AGENT_APPROVES_FIRST);
    const failed = {
      ...updating,
      summary: { ...updating.summary, checks_failed: true },
    };
    expect(approveBlock(failed, "tanim")).toBe(CHECKS_FAILED);
  });

  it("blocks a human's Approve on a marked task until a reviewer agent approved the current version", () => {
    const waiting = proposalDetail(
      proposalSummary("P-4", { human_review: mark }),
      { agent_approved: false },
    );
    expect(approveBlock(waiting, "tanim")).toBe(AGENT_APPROVES_FIRST);
    expect(AGENT_APPROVES_FIRST).toBe("A reviewer agent approves first.");
    const approved = proposalDetail(
      proposalSummary("P-4", { human_review: mark }),
      { agent_approved: true },
    );
    expect(approveBlock(approved, "tanim")).toBeNull();
  });

  it("lets a human approve a task of the main agent's area first, since no reviewer agent reviews it", () => {
    const main = proposalDetail(
      proposalSummary("P-4", { human_review: mark }),
      { agent_approved: false, main_task: true },
    );
    expect(approveBlock(main, "tanim")).toBeNull();
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
    expect(approveBlock(proposal(), "tanim")).toBeNull();
  });

  it("blocks Approve while the signed-in human's latest verdict on the current version is an approval", () => {
    const approved = proposalDetail(
      proposalSummary("P-4", { current_version: 5 }),
      {
        reviews: [
          review(5, "review/security", "approve"),
          review(5, "human:tanim", "approve"),
        ],
      },
    );
    expect(approveBlock(approved, "tanim")).toBe("You approved v5.");
    expect(ownApprovalLine(5)).toBe("You approved v5.");
    expect(approveBlock(approved, "pptx704")).toBeNull();
    expect(approveBlock(approved, null)).toBeNull();
    expect(integrateBlock(approved, "r8")).toBeNull();
  });

  it("lets the human approve again after their own request for changes, and on a new version", () => {
    const first = review(5, "human:tanim", "approve");
    const changed = proposalDetail(
      proposalSummary("P-4", { current_version: 5 }),
      {
        reviews: [
          first,
          review(5, "human:tanim", "request_changes", {
            created_at: first.created_at + 1,
          }),
        ],
      },
    );
    expect(approveBlock(changed, "tanim")).toBeNull();
    const newer = proposalDetail(
      proposalSummary("P-4", { current_version: 6 }),
      {
        reviews: [first],
      },
    );
    expect(approveBlock(newer, "tanim")).toBeNull();
  });

  it("blocks Approve while a check run of either stage runs on the current version", () => {
    const running = (stage: "proposal" | "integration") =>
      proposalDetail(proposalSummary("P-4", { current_version: 5 }), {
        checks: [
          checkRun(163, { stage, state: "running", finished_at: null }),
          checkRun(152),
        ],
      });
    expect(approveBlock(running("proposal"), "tanim")).toBe(
      "A check runs on v5. Approve when it ends.",
    );
    expect(approveBlock(running("integration"), "tanim")).toBe(
      checkRunsLine(5),
    );
    const ended = proposalDetail(
      proposalSummary("P-4", { current_version: 5 }),
      {
        checks: [checkRun(163, { stage: "integration", state: "stopped" })],
      },
    );
    expect(approveBlock(ended, "tanim")).toBeNull();
  });

  it("reads the runs only of a read that selected the current version", () => {
    const earlier = proposalDetail(
      proposalSummary("P-4", { current_version: 5 }),
      {
        selected: proposalVersion(4),
        checks: [checkRun(9, { state: "running", finished_at: null })],
      },
    );
    expect(approveBlock(earlier, "tanim")).toBeNull();
  });

  it("names the failed checks first, then the human's own approval, then a running check, then the agent", () => {
    const all = (fields: Partial<ProposalSummary>, runs = true) =>
      proposalDetail(
        proposalSummary("P-4", {
          current_version: 5,
          human_review: mark,
          ...fields,
        }),
        {
          agent_approved: false,
          reviews: [review(5, "human:tanim", "approve")],
          checks: runs
            ? [checkRun(163, { state: "running", finished_at: null })]
            : [],
        },
      );
    expect(approveBlock(all({ checks_failed: true }), "tanim")).toBe(
      CHECKS_FAILED,
    );
    expect(approveBlock(all({}), "tanim")).toBe(ownApprovalLine(5));
    expect(approveBlock(all({}), "pptx704")).toBe(checkRunsLine(5));
    expect(approveBlock(all({}, false), "pptx704")).toBe(AGENT_APPROVES_FIRST);
  });
});

describe("showsApproveLine", () => {
  it("shows the failed checks line once, under both buttons, while Integrate shows", () => {
    expect(showsApproveLine(CHECKS_FAILED, CHECKS_FAILED, true)).toBe(false);
    expect(showsApproveLine(CHECKS_FAILED, CHECKS_FAILED, false)).toBe(true);
    expect(showsApproveLine(CHECKS_FAILED, null, true)).toBe(true);
  });

  it("shows the line of a running update once, under both buttons, while Integrate shows", () => {
    const line = updateRunsLine(3);
    expect(showsApproveLine(line, line, true)).toBe(false);
    expect(showsApproveLine(line, line, false)).toBe(true);
    expect(showsApproveLine(line, CHECKS_FAILED, true)).toBe(true);
  });

  it("keeps every other line under Approve", () => {
    expect(
      showsApproveLine(AGENT_APPROVES_FIRST, AGENT_APPROVES_FIRST, true),
    ).toBe(true);
    expect(showsApproveLine(null, CHECKS_FAILED, true)).toBe(false);
  });
});
