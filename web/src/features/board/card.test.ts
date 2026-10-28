import { describe, expect, it } from "vitest";
import type { BoardProposal } from "@/api/board";
import {
  blockedText,
  checklistText,
  kindText,
  leadCheckMark,
  needsText,
  planWaitText,
  proposalLook,
  proposalText,
  taskPath,
} from "./card";

const proposal = (
  state: BoardProposal["state"],
  revision: string | null = null,
): BoardProposal => ({
  id: "P-3",
  state,
  version: 1,
  checks_failed: false,
  revision,
  review_plan: null,
});

describe("taskPath", () => {
  it("is the task dialog route", () => {
    expect(taskPath("demo", "T-12")).toBe("/p/demo/t/T-12");
    expect(taskPath("demo", "T-12", "?label=ui")).toBe(
      "/p/demo/t/T-12?label=ui",
    );
  });
});

describe("checklistText", () => {
  it("counts plain steps and linked items together", () => {
    expect(
      checklistText({
        checklist: {
          steps_done: 1,
          steps_total: 3,
          linked_done: 1,
          linked_total: 2,
        },
      }),
    ).toBe("2/5");
  });

  it("is null for a task with no checklist", () => {
    expect(
      checklistText({
        checklist: {
          steps_done: 0,
          steps_total: 0,
          linked_done: 0,
          linked_total: 0,
        },
      }),
    ).toBeNull();
  });
});

describe("needsText", () => {
  it("lists the unfinished prerequisites in Backlog", () => {
    expect(
      needsText({
        status: "backlog",
        blocked: false,
        blocked_by: ["T-3", "T-9"],
      }),
    ).toBe("needs T-3, T-9");
  });

  it("is null outside Backlog, for a blocked task, or with no prerequisite", () => {
    expect(
      needsText({ status: "todo", blocked: false, blocked_by: ["T-3"] }),
    ).toBeNull();
    expect(
      needsText({ status: "backlog", blocked: true, blocked_by: ["T-3"] }),
    ).toBeNull();
    expect(
      needsText({ status: "backlog", blocked: false, blocked_by: [] }),
    ).toBeNull();
  });
});

describe("blockedText", () => {
  it("lists the unfinished prerequisites of a blocked task", () => {
    expect(blockedText({ blocked: true, blocked_by: ["T-3", "T-9"] })).toBe(
      "Blocked by T-3, T-9",
    );
    expect(blockedText({ blocked: true, blocked_by: [] })).toBe("Blocked");
    expect(blockedText({ blocked: false, blocked_by: ["T-3"] })).toBeNull();
  });
});

describe("proposalText", () => {
  it("names the proposal and its state", () => {
    expect(proposalText(proposal("changes_requested"))).toBe(
      "P-3 · changes requested",
    );
    expect(proposalText(proposal("conflict"))).toBe("P-3 · conflict");
    expect(proposalText(proposal("open"))).toBe("P-3 · waiting for review");
    expect(proposalText(proposal("approved"))).toBe("P-3 · approved");
    expect(proposalText(proposal("integrated", "r8"))).toBe(
      "P-3 · integrated as r8",
    );
    expect(proposalText(proposal("integrated"))).toBe("P-3 · integrated");
  });
});

describe("proposalLook", () => {
  it("draws attention for changes requested and conflict", () => {
    expect(proposalLook(proposal("changes_requested"))).toBe("attention");
    expect(proposalLook(proposal("conflict"))).toBe("attention");
    expect(proposalLook(proposal("open"))).toBe("active");
    expect(proposalLook(proposal("approved"))).toBe("active");
    expect(proposalLook(proposal("integrated", "r8"))).toBe("merged");
  });
});

describe("tasks without a proposal", () => {
  it("names the kind and marks a reported task in Review", () => {
    const card = {
      kind: "no_proposal" as const,
      status: "review" as const,
      report: "Drew the board.",
    };
    expect(kindText(card)).toBe("no proposal");
    expect(kindText({ kind: "proposal" })).toBeNull();
    expect(leadCheckMark(card)).toEqual({
      text: "Waits for lead check",
      note: "Drew the board.",
    });
    expect(leadCheckMark({ ...card, status: "in_progress" })).toBeNull();
    expect(leadCheckMark({ ...card, report: null })).toBeNull();
    expect(leadCheckMark({ ...card, kind: "proposal" })).toBeNull();
  });
});

describe("human tasks", () => {
  it("names a human task", () => {
    expect(kindText({ kind: "human" })).toBe("human task");
    expect(
      leadCheckMark({ kind: "human", status: "review", report: "x" }),
    ).toBeNull();
  });
});

describe("planWaitText", () => {
  const waiting: BoardProposal = {
    ...proposal("open"),
    review_plan: {
      plan: "rp-1",
      deadline_at: new Date(2026, 9, 21, 15, 0).getTime(),
      wait_ended: false,
      awaited: ["T-5"],
      proposals: ["P-3"],
    },
  };
  const now = new Date(2026, 9, 21, 14, 0).getTime();

  it("reads an open proposal of a plan as waiting for its batch", () => {
    expect(proposalText(waiting)).toBe("P-3 · waiting for a batch");
    expect(planWaitText(waiting, now)).toBe("rp-1 waits for T-5 until 15:00");
  });

  it("is null for a proposal in no plan", () => {
    expect(planWaitText(proposal("open"), now)).toBeNull();
    expect(proposalText(proposal("open"))).toBe("P-3 · waiting for review");
  });
});
