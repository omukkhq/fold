import { describe, expect, it } from "vitest";
import type { Gate, ProposalDetail } from "@/api/proposals";
import { detail } from "../../../fixtures/proposals";
import { gateText, orderGates, waitingSummary } from "./gates";

function proposal(
  id: string,
  fields: Partial<ProposalDetail> = {},
): ProposalDetail {
  const found = detail("demo", id);
  if (!found) throw new Error(`no fixture ${id}`);
  return { ...structuredClone(found), ...fields };
}

const clock = (at: number) => `@${String(at)}`;

describe("orderGates", () => {
  it("orders gates as the integration checks them, with the human review after the approval", () => {
    const gates: Gate[] = [
      {
        kind: "merge",
        state: "met",
        main: "r8",
        conflicting_files: 0,
        checked_at: 1,
      },
      { kind: "prerequisites", state: "met", unfinished: [], total: 0 },
      { kind: "integrator", state: "met", agents: ["review"] },
      {
        kind: "human_review",
        state: "waiting_human",
        reviewer: "tanim",
        marked_by: "backend",
        reason: "",
      },
      {
        kind: "approval",
        state: "met",
        version: 2,
        approvers: ["review/quality"],
        latest_verdict: "approve",
      },
    ];
    expect(orderGates(gates).map((g) => g.kind)).toEqual([
      "approval",
      "human_review",
      "integrator",
      "prerequisites",
      "merge",
    ]);
  });
});

describe("gateText", () => {
  it("describes an approval gate in each state", () => {
    const met: Gate = {
      kind: "approval",
      state: "met",
      version: 2,
      approvers: ["review/quality", "human:tanim"],
      latest_verdict: "approve",
    };
    expect(gateText(met, clock)).toBe(
      "Approved on v2 by review/quality and tanim",
    );
    expect(gateText({ ...met, approvers: ["a", "b", "c"] }, clock)).toBe(
      "Approved on v2 by a, b and c",
    );
    expect(
      gateText(
        {
          ...met,
          state: "unmet",
          approvers: [],
          latest_verdict: "request_changes",
        },
        clock,
      ),
    ).toBe("Changes requested on v2; it needs an approval");
    expect(
      gateText(
        { ...met, state: "unmet", approvers: [], latest_verdict: null },
        clock,
      ),
    ).toBe("No approval on v2 yet");
  });

  it("describes the human review waiting, met and for any human", () => {
    const gate: Gate = {
      kind: "human_review",
      state: "waiting_human",
      reviewer: "tanim",
      marked_by: "backend",
      reason: "schema work for a junior task",
    };
    expect(gateText(gate, clock)).toBe(
      'Human review by tanim · marked by backend: "schema work for a junior task"',
    );
    expect(
      gateText(
        {
          ...gate,
          state: "met",
          reviewer: null,
          marked_by: "human:alex",
          reason: " ",
        },
        clock,
      ),
    ).toBe("Approved by any human · marked by alex");
  });

  it("describes the integrator tag holders", () => {
    expect(
      gateText({ kind: "integrator", state: "met", agents: ["review"] }, clock),
    ).toBe("Caller needs the integrator tag: review has it");
    expect(
      gateText(
        { kind: "integrator", state: "met", agents: ["review", "lead"] },
        clock,
      ),
    ).toBe("Caller needs the integrator tag: review and lead have it");
    expect(
      gateText({ kind: "integrator", state: "unmet", agents: [] }, clock),
    ).toBe("Caller needs the integrator tag: no agent has it");
  });

  it("describes the prerequisites", () => {
    const gate: Gate = {
      kind: "prerequisites",
      state: "met",
      unfinished: [],
      total: 0,
    };
    expect(gateText(gate, clock)).toBe("Prerequisites Done: none needed");
    expect(gateText({ ...gate, total: 2 }, clock)).toBe(
      "Prerequisites Done: all 2",
    );
    expect(
      gateText(
        { ...gate, state: "unmet", total: 2, unfinished: ["T-4", "T-9"] },
        clock,
      ),
    ).toBe("Prerequisites not Done: T-4, T-9");
  });

  it("describes the merge with main", () => {
    const gate: Gate = {
      kind: "merge",
      state: "met",
      main: "r8",
      conflicting_files: 0,
      checked_at: 5,
    };
    expect(gateText(gate, clock)).toBe("Merges cleanly onto r8 (checked @5)");
    expect(gateText({ ...gate, checked_at: null }, clock)).toBe(
      "Merges cleanly onto r8 (not checked yet)",
    );
    expect(
      gateText({ ...gate, state: "unmet", conflicting_files: 1 }, clock),
    ).toBe("Does not merge onto r8: 1 file conflicts (checked @5)");
    expect(
      gateText({ ...gate, state: "unmet", conflicting_files: 5 }, clock),
    ).toBe("Does not merge onto r8: 5 files conflict (checked @5)");
  });
});

describe("waitingSummary", () => {
  it("waits on the named human while the human review waits", () => {
    expect(waitingSummary(proposal("P-4"))).toBe(
      "Waiting for tanim. When tanim approves v2, review/quality integrates, or tanim can integrate here.",
    );
    const anyHuman = proposal("P-4", { review_claim: null });
    anyHuman.gates = anyHuman.gates.map((g) =>
      g.kind === "human_review" ? { ...g, reviewer: null } : g,
    );
    expect(waitingSummary(anyHuman)).toBe(
      "Waiting for a human. When a human approves v2, the review agent integrates, or a human can integrate here.",
    );
  });

  it("says the report went to the author of a conflicted proposal", () => {
    expect(waitingSummary(proposal("P-6"))).toBe(
      "fold sent the report to frontend/charts. The task is back in In Progress until a new version arrives.",
    );
  });

  it("waits on the author after a request for changes", () => {
    expect(waitingSummary(proposal("P-3"))).toBe(
      "Waiting for frontend/styles to send a new version.",
    );
  });

  it("names the first unmet gate while the proposal is with the reviewer", () => {
    const p = proposal("P-3", { with: "reviewer", latest_verdict: null });
    expect(waitingSummary(p)).toBe("Waiting for a review of v1.");
    p.gates = p.gates.map((g): Gate => {
      if (g.kind === "approval") return { ...g, state: "met" };
      if (g.kind === "prerequisites")
        return { ...g, state: "unmet", total: 1, unfinished: ["T-4"] };
      return g;
    });
    expect(waitingSummary(p)).toBe("Waiting for T-4 to be Done.");
    p.gates = p.gates.map((g): Gate => {
      if (g.kind === "prerequisites") return { ...g, state: "met" };
      if (g.kind === "integrator") return { ...g, state: "unmet", agents: [] };
      return g;
    });
    expect(waitingSummary(p)).toBe(
      "Waiting for a human to give an agent the integrator tag, or to integrate here.",
    );
    p.gates = p.gates.map((g): Gate => {
      if (g.kind === "integrator") return { ...g, state: "met" };
      if (g.kind === "merge") return { ...g, state: "unmet" };
      return g;
    });
    expect(waitingSummary(p)).toBe("Waiting on the gates above.");
  });

  it("says every gate is met and who integrates next", () => {
    const p = proposal("P-3", { with: "reviewer", latest_verdict: "approve" });
    p.gates = p.gates.map(
      (g): Gate => (g.kind === "approval" ? { ...g, state: "met" } : g),
    );
    expect(waitingSummary(p)).toBe(
      "Every gate is met. Next, the review agent integrates, or a human can integrate here.",
    );
    p.gates = p.gates.filter((g) => g.kind !== "integrator");
    expect(waitingSummary(p)).toBe(
      "Every gate is met. Next, an integrator integrates, or a human can integrate here.",
    );
  });

  it("waits on nothing once integrated and gives the reason of a closed proposal", () => {
    expect(waitingSummary(proposal("P-5"))).toBeNull();
    expect(waitingSummary(proposal("P-8"))).toBe(
      "Closed without integration: Split into a smaller change after T-1",
    );
    expect(waitingSummary(proposal("P-8", { close_reason: null }))).toBe(
      "Closed without integration.",
    );
  });
});
