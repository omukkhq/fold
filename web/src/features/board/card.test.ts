import { describe, expect, it } from "vitest";
import type { BoardProposal } from "@/api/board";
import {
  integrationWait,
  integrationWaitDetail,
  blockedText,
  checklistText,
  kindText,
  leadCheckMark,
  needsText,
  planWaitText,
  proposalLook,
  proposalText,
  reviewingText,
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
  review_claim: null,
  queue: null,
  waits_for: null,
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

  it("names at most three prerequisites and counts the rest, so the line stays short", () => {
    const ids = (count: number) =>
      Array.from({ length: count }, (_, index) => `T-${String(148 + index)}`);
    const needs = (count: number) =>
      needsText({ status: "backlog", blocked: false, blocked_by: ids(count) });
    expect(needs(0)).toBeNull();
    expect(needs(1)).toBe("needs T-148");
    expect(needs(3)).toBe("needs T-148, T-149, T-150");
    expect(needs(4)).toBe("needs T-148, T-149, T-150 +1 more");
    expect(needs(43)).toBe("needs T-148, T-149, T-150 +40 more");
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
    expect(
      blockedText({
        blocked: true,
        blocked_by: ["T-1", "T-2", "T-3", "T-4", "T-5"],
      }),
    ).toBe("Blocked by T-1, T-2, T-3 +2 more");
    expect(blockedText({ blocked: false, blocked_by: ["T-3"] })).toBeNull();
  });
});

describe("proposalText", () => {
  it("names the proposal and its state", () => {
    expect(proposalText(proposal("changes_requested"), 0)).toBe(
      "P-3 · changes requested",
    );
    expect(proposalText(proposal("conflict"), 0)).toBe("P-3 · conflict");
    expect(proposalText(proposal("open"), 0)).toBe("P-3 · waiting for review");
    expect(proposalText(proposal("approved"), 0)).toBe("P-3 · approved");
    expect(proposalText(proposal("integrated", "r8"), 0)).toBe(
      "P-3 · integrated as r8",
    );
    expect(proposalText(proposal("integrated"), 0)).toBe("P-3 · integrated");
  });

  it("says an open proposal is being reviewed by the reviewer that holds its claim, also in a review plan", () => {
    const held = { ...proposal("open"), review_claim: "review-2" };
    expect(proposalText(held, 0)).toBe("P-3 · being reviewed by @review-2");
    expect(
      proposalText(
        {
          ...held,
          review_plan: {
            plan: "rp-1",
            awaited: ["T-4"],
            deadline_at: 10,
            wait_ended: false,
            proposals: ["P-3"],
          },
        },
        0,
      ),
    ).toBe("P-3 · being reviewed by @review-2");
    expect(reviewingText(held)).toBe("being reviewed by @review-2");
    expect(
      reviewingText({ state: "approved", review_claim: "review-2" }),
    ).toBeNull();
    expect(reviewingText(proposal("open"))).toBeNull();
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
    expect(proposalText(waiting, 0)).toBe("P-3 · waiting for a batch");
    expect(planWaitText(waiting, now)).toBe("rp-1 waits for T-5 until 15:00");
  });

  it("is null for a proposal in no plan", () => {
    expect(planWaitText(proposal("open"), now)).toBeNull();
    expect(proposalText(proposal("open"), 0)).toBe("P-3 · waiting for review");
  });
});

describe("integration wait of an approved proposal", () => {
  const now = 1_000_000;
  const queued: BoardProposal = {
    ...proposal("approved"),
    queue: {
      position: 2,
      ahead: "P-1",
      batch: null,
      estimated_start: now + 10 * 60_000,
      integrating_now: false,
    },
  };
  const waiting: BoardProposal = {
    ...proposal("approved"),
    waits_for: { reason: "automatic_off" },
  };

  it("names the queue place and the wait reason with the text module, never `approved` alone", () => {
    expect(proposalText(queued, now)).toBe("P-3 · 2nd in line");
    expect(integrationWaitDetail(queued, now)).toBe(
      "after P-1 · starts in about 10 min",
    );
    expect(proposalText(waiting, now)).toBe(
      "P-3 · Automatic integration is off",
    );
    expect(integrationWaitDetail(waiting, now)).toBe("a human integrates it");
    expect(integrationWait(waiting, now)?.state).toBe("waiting");
  });

  it("has no detail when the title says it all, and keeps `approved` only when the daemon names neither", () => {
    const next: BoardProposal = {
      ...proposal("approved"),
      queue: {
        position: 1,
        ahead: null,
        batch: null,
        estimated_start: null,
        integrating_now: false,
      },
    };
    expect(integrationWaitDetail(next, now)).toBeNull();
    expect(proposalText(proposal("approved"), now)).toBe("P-3 · approved");
  });

  it("shows no queue text for an unapproved proposal", () => {
    const open: BoardProposal = {
      ...proposal("open"),
      waits_for: { reason: "not_approved" },
    };
    expect(integrationWait(open, now)).toBeNull();
    expect(integrationWaitDetail(open, now)).toBeNull();
    expect(proposalText(open, now)).toBe("P-3 · waiting for review");
  });
});
