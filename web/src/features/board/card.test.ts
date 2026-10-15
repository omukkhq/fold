import { describe, expect, it } from "vitest";
import type { BoardProposal } from "@/api/board";
import {
  blockedText,
  blocksText,
  checklistText,
  humanReviewText,
  messagesText,
  needsText,
  proposalLook,
  proposalText,
  taskPath,
} from "./card";

const proposal = (
  state: BoardProposal["state"],
  revision: string | null = null,
): BoardProposal => ({ id: "P-3", state, revision });

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

describe("blocksText", () => {
  it("counts the open tasks that wait for this one", () => {
    expect(blocksText({ blocks: 2 })).toBe("blocks 2");
    expect(blocksText({ blocks: 0 })).toBeNull();
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

describe("messagesText", () => {
  it("counts the messages that name the task", () => {
    expect(messagesText({ messages: 3 })).toBe("3");
    expect(messagesText({ messages: 0 })).toBeNull();
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

describe("humanReviewText", () => {
  it("names the reviewer when there is one", () => {
    expect(
      humanReviewText({ reviewer: "tanim", reason: "auth", set_by: "planner" }),
    ).toBe("Human review · tanim");
    expect(
      humanReviewText({ reviewer: null, reason: "auth", set_by: "planner" }),
    ).toBe("Human review");
  });
});
