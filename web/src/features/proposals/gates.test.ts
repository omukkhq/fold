import { describe, expect, it } from "vitest";
import type {
  Gate,
  ProposalDetail,
  ProposalSummary,
  ProposalTask,
} from "@/api/proposals";
import {
  conflictReport,
  humanGates,
  proposalDetail,
  proposalSummary,
  review,
} from "@/test/proposals";
import { gateText, shownGates, waitingSummary } from "./gates";

const mark = {
  reviewer: "tanim",
  reason: "schema work for a junior task",
  set_by: "backend",
};

function proposal(
  summary: Partial<ProposalSummary> = {},
  fields: Partial<ProposalDetail> = {},
): ProposalDetail {
  return proposalDetail(
    proposalSummary("P-4", { current_version: 2, ...summary }),
    fields,
  );
}

function gate(
  name: Gate["gate"],
  status: Gate["status"] = "met",
  code: string | null = null,
): Gate {
  return { gate: name, status, code };
}

const task = (id: string, status: ProposalTask["status"]): ProposalTask => ({
  task: id,
  title: id,
  description: "",
  status,
});

describe("shownGates", () => {
  it("orders the gates as the integration checks them and leaves out a met open or version gate", () => {
    const gates = [...humanGates()].reverse();
    expect(shownGates(gates).map((g) => g.gate)).toEqual([
      "caller",
      "approval",
      "human_review",
      "prerequisites",
      "merge",
    ]);
    expect(
      shownGates(
        humanGates({
          open: { status: "unmet", code: "proposal_not_open" },
          version: { status: "unmet", code: "stale_version" },
        }),
      ).map((g) => g.gate),
    ).toEqual([
      "open",
      "version",
      "caller",
      "approval",
      "human_review",
      "prerequisites",
      "merge",
    ]);
  });
});

describe("gateText", () => {
  it("describes the open, version and caller gates in each state", () => {
    const p = proposal();
    expect(gateText(gate("open"), p, null)).toBe("The proposal is open");
    expect(gateText(gate("open", "unmet"), p, null)).toBe(
      "The proposal is not open",
    );
    expect(gateText(gate("version"), p, null)).toBe(
      "v2 is the current version",
    );
    expect(gateText(gate("version", "unmet"), p, null)).toBe(
      "v2 is not the version on screen",
    );
    expect(gateText(gate("caller"), p, null)).toBe(
      "You can integrate as a named human",
    );
    expect(
      gateText(gate("caller", "unmet", "author_cannot_integrate"), p, null),
    ).toBe("Your agent wrote this proposal, so you cannot integrate it");
    expect(
      gateText(gate("caller", "unmet", "integrator_required"), p, null),
    ).toBe("You cannot integrate this proposal");
  });

  it("names the approvers of the version, and says when integrating here approves", () => {
    const approved = proposal(
      {},
      {
        reviews: [
          review(2, "review/quality", "approve", { created_at: 1 }),
          review(2, "human:tanim", "approve", { created_at: 2 }),
        ],
      },
    );
    expect(gateText(gate("approval"), approved, null)).toBe(
      "Approved on v2 by review/quality and tanim",
    );
    const changes = proposal(
      { latest_verdict: "request_changes" },
      { reviews: [review(2, "review/security", "request_changes")] },
    );
    expect(gateText(gate("approval"), changes, null)).toBe(
      "Changes requested on v2; integrating here approves it",
    );
    expect(gateText(gate("approval"), proposal(), null)).toBe(
      "No approval on v2 yet; integrating here approves it",
    );
    expect(
      gateText(
        gate("approval", "unmet", "changes_requested"),
        proposal(),
        null,
      ),
    ).toBe("Changes requested on v2; it needs an approval");
    expect(
      gateText(gate("approval", "unmet", "not_approved"), proposal(), null),
    ).toBe("No approval on v2 yet; it needs an approval");
  });

  it("describes the human review mark with who set it and why", () => {
    expect(gateText(gate("human_review"), proposal(), null)).toBe(
      "No human review asked for",
    );
    const marked = proposal({ human_review: mark });
    expect(gateText(gate("human_review"), marked, null)).toBe(
      'Human review by tanim · marked by backend: "schema work for a junior task"',
    );
    expect(
      gateText(
        gate("human_review"),
        proposal(
          {
            human_review: { reviewer: null, reason: " ", set_by: "human:ana" },
            human_approved: true,
          },
          { reviews: [review(2, "human:ana", "approve")] },
        ),
        null,
      ),
    ).toBe("Approved by any human · marked by ana");
  });

  it("lists the prerequisites that are not Done", () => {
    expect(gateText(gate("prerequisites"), proposal(), null)).toBe(
      "Prerequisites Done: none needed",
    );
    const done = proposal({}, { prerequisites: [task("T-1", "done")] });
    expect(gateText(gate("prerequisites"), done, null)).toBe(
      "Prerequisites Done: all 1",
    );
    const open = proposal(
      {},
      { prerequisites: [task("T-1", "done"), task("T-2", "review")] },
    );
    expect(
      gateText(
        gate("prerequisites", "unmet", "prerequisites_not_done"),
        open,
        null,
      ),
    ).toBe("Prerequisites not Done: T-2");
    expect(
      gateText(
        gate("prerequisites", "unmet", "prerequisites_not_done"),
        done,
        null,
      ),
    ).toBe("Prerequisites not Done");
  });

  it("says the merge is checked at integration, or names the conflict the last attempt found", () => {
    const p = proposal();
    expect(gateText(gate("merge", "checked_at_integration"), p, "r8")).toBe(
      "Merge onto r8 is checked at integration",
    );
    expect(gateText(gate("merge", "checked_at_integration"), p, null)).toBe(
      "The merge is checked at integration",
    );
    expect(gateText(gate("merge"), p, null)).toBe("Merges cleanly");
    expect(gateText(gate("merge", "unmet"), p, null)).toBe(
      "Does not merge onto main",
    );
    const conflicted = proposal(
      { conflicted: true },
      { conflict: conflictReport("P-4", 2) },
    );
    expect(
      gateText(gate("merge", "checked_at_integration"), conflicted, "r8"),
    ).toBe("Does not merge onto r8: 5 files conflict");
    const one = conflictReport("P-4", 2);
    one.files = one.files.slice(0, 1);
    expect(
      gateText(
        gate("merge", "checked_at_integration"),
        proposal({}, { conflict: one }),
        null,
      ),
    ).toBe("Does not merge onto r8: 1 file conflicts");
  });
});

describe("waitingSummary", () => {
  it("says nothing for an integrated proposal and gives a closed one's reason", () => {
    expect(
      waitingSummary(proposal({ state: "integrated", turn: null })),
    ).toBeNull();
    expect(
      waitingSummary(
        proposal({
          state: "closed",
          turn: null,
          close_reason: "superseded by P-9",
        }),
      ),
    ).toBe("Closed without integration: superseded by P-9");
    expect(waitingSummary(proposal({ state: "closed", turn: null }))).toBe(
      "Closed without integration.",
    );
  });

  it("sends a conflicted or returned proposal back to its author", () => {
    expect(
      waitingSummary(proposal({ conflicted: true, turn: "with_author" })),
    ).toBe(
      "fold sent the report to backend/db. The task is back in In Progress until a new version arrives.",
    );
    expect(waitingSummary(proposal({ turn: "with_author" }))).toBe(
      "Waiting for backend/db to send a new version.",
    );
  });

  it("waits for the named human until a human approves, naming who integrates", () => {
    const approved = [review(2, "review/quality", "approve")];
    expect(
      waitingSummary(
        proposal(
          { human_review: mark, latest_verdict: "approve" },
          {
            reviews: approved,
            review_claim: {
              holder: "review/quality",
              claimed_at: 1,
              renewed_at: 2,
              stale: false,
            },
          },
        ),
      ),
    ).toBe(
      "Waiting for tanim. When tanim approves v2, review/quality integrates, or tanim can integrate here.",
    );
    expect(
      waitingSummary(
        proposal(
          { human_review: { ...mark, reviewer: null } },
          { reviews: approved },
        ),
      ),
    ).toBe(
      "Waiting for a human. When a human approves v2, an integrator integrates, or a human can integrate here.",
    );
  });

  it("waits for a review, then for the prerequisites, then for an integrator", () => {
    expect(waitingSummary(proposal())).toBe("Waiting for a review of v2.");
    const approved = [review(2, "review/quality", "approve")];
    expect(
      waitingSummary(
        proposal(
          { latest_verdict: "approve" },
          {
            reviews: approved,
            prerequisites: [task("T-2", "in_progress")],
            gates: humanGates({
              prerequisites: {
                status: "unmet",
                code: "prerequisites_not_done",
              },
            }),
          },
        ),
      ),
    ).toBe("Waiting for T-2 to be Done.");
    expect(
      waitingSummary(
        proposal(
          { latest_verdict: "approve" },
          {
            reviews: approved,
            gates: humanGates({
              prerequisites: {
                status: "unmet",
                code: "prerequisites_not_done",
              },
            }),
          },
        ),
      ),
    ).toBe("Waiting for the prerequisites to be Done.");
    expect(
      waitingSummary(
        proposal(
          { latest_verdict: "approve" },
          {
            reviews: approved,
            gates: humanGates({
              caller: { status: "unmet", code: "author_cannot_integrate" },
            }),
          },
        ),
      ),
    ).toBe("Waiting on the gates above.");
    expect(
      waitingSummary(
        proposal(
          { review_claim: "review/quality", latest_verdict: "approve" },
          { reviews: approved },
        ),
      ),
    ).toBe(
      "Every gate is met. Next, review/quality integrates, or a human can integrate here.",
    );
  });
});
