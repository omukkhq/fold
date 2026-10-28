import { describe, expect, it } from "vitest";
import type { BoardCard, BoardProposal } from "@/api/board";
import { boardCard } from "@/test/payloads";
import { cardChip, integratedText, metaFact } from "./chip";

const proposal = (
  state: BoardProposal["state"],
  over: Partial<BoardProposal> = {},
): BoardProposal => ({
  id: "P-3",
  state,
  version: 1,
  checks_failed: false,
  revision: null,
  review_plan: null,
  ...over,
});

const chipText = (over: Partial<BoardCard>) =>
  cardChip(boardCard("T-1", { status: "in_progress", ...over }))?.text ?? null;

const BLOCKER = { reason: "Wait", set_by: "human:tanim", set_at: 0 };

describe("cardChip", () => {
  it("puts a question before a review, a review before a blocker", () => {
    expect(
      chipText({
        attention: [
          { kind: "human_review", closable: false },
          { kind: "question", closable: true },
        ],
        blocker: BLOCKER,
      }),
    ).toBe("Question for you");
    expect(
      chipText({
        attention: [{ kind: "human_review", closable: false }],
        blocker: BLOCKER,
      }),
    ).toBe("Your review");
    expect(chipText({ blocker: BLOCKER, blocked: true })).toBe("Blocker");
  });

  it("names the prerequisite of a blocked card, also when it stalls", () => {
    const chip = cardChip(
      boardCard("T-8", {
        status: "in_progress",
        blocked: true,
        blocked_by: ["T-9"],
        attention: [{ kind: "stalled", closable: false }],
      }),
    );
    expect(chip?.text).toBe("Blocked by T-9");
    expect(chip?.tone).toBe("bad");
  });

  it("shows a stall, then a stale claim, in gold", () => {
    const stalled = cardChip(
      boardCard("T-1", {
        attention: [
          { kind: "stale_claim", closable: false },
          { kind: "stalled", closable: false },
        ],
      }),
    );
    expect(stalled?.text).toBe("Stalled · needs you");
    expect(stalled?.tone).toBe("warn");
    expect(
      chipText({ attention: [{ kind: "stale_claim", closable: false }] }),
    ).toBe("Stale claim · needs you");
  });

  it("names a proposal back with its author", () => {
    expect(chipText({ proposal: proposal("conflict") })).toBe("P-3 conflict");
    expect(chipText({ proposal: proposal("changes_requested") })).toBe(
      "P-3 changes requested",
    );
  });

  it("shows a wait on others in the neutral tone", () => {
    const open = cardChip(boardCard("T-1", { proposal: proposal("open") }));
    expect(open?.text).toBe("P-3 waits for review");
    expect(open?.tone).toBe("neutral");
    expect(
      chipText({
        proposal: proposal("open", {
          review_plan: {
            plan: "rp-1",
            awaited: ["T-4"],
            deadline_at: 10,
            wait_ended: false,
            proposals: ["P-3"],
          },
        }),
      }),
    ).toBe("P-3 waiting for a batch");
    expect(
      chipText({
        kind: "no_proposal",
        status: "review",
        report: "Drew the icons.",
      }),
    ).toBe("Waits for lead check");
  });

  it("gives an approved proposal, a quiet card and a Done card no chip", () => {
    expect(chipText({ proposal: proposal("approved") })).toBeNull();
    expect(chipText({})).toBeNull();
    expect(
      cardChip(
        boardCard("T-1", {
          status: "done",
          attention: [{ kind: "question", closable: true }],
        }),
      ),
    ).toBeNull();
  });
});

describe("integratedText", () => {
  it("names the revision of an integrated proposal only", () => {
    expect(
      integratedText({ proposal: proposal("integrated", { revision: "r4" }) }),
    ).toBe("integrated as r4");
    expect(integratedText({ proposal: proposal("integrated") })).toBeNull();
    expect(integratedText({ proposal: null })).toBeNull();
  });
});

describe("metaFact", () => {
  const fact = (over: Partial<BoardCard>) => metaFact(boardCard("T-1", over));

  it("names the worker and nothing else", () => {
    expect(fact({ worker: "api-routes", blocks: 2 })).toBe("api-routes");
  });

  it("names where a Done task reached main, else its checklist", () => {
    expect(
      fact({
        status: "done",
        proposal: proposal("integrated", { revision: "r4" }),
      }),
    ).toBe("integrated as r4");
    expect(
      fact({
        status: "done",
        checklist: {
          steps_done: 3,
          steps_total: 3,
          linked_done: 0,
          linked_total: 0,
        },
      }),
    ).toBe("checklist 3/3");
    expect(fact({ status: "done" })).toBeNull();
  });

  it("takes the first of the prerequisites, the kind, the blocked tasks and the checklist", () => {
    expect(
      fact({
        status: "backlog",
        blocked_by: ["T-2"],
        kind: "human",
        blocks: 1,
      }),
    ).toBe("needs T-2");
    expect(fact({ kind: "human", blocks: 1 })).toBe("human task");
    expect(fact({ blocks: 2 })).toBe("blocks 2");
    expect(
      fact({
        checklist: {
          steps_done: 1,
          steps_total: 2,
          linked_done: 0,
          linked_total: 0,
        },
      }),
    ).toBe("checklist 1/2");
    expect(fact({})).toBeNull();
  });
});
