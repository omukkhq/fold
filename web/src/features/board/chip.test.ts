import { describe, expect, it } from "vitest";
import {
  Activity,
  CirclePause,
  CircleStop,
  Clock,
  CloudOff,
  Gauge,
  Hand,
  Hourglass,
} from "lucide-react";
import type { BoardCard, BoardProposal } from "@/api/board";
import { boardCard } from "@/test/payloads";
import { SLOT_CHIP_TEXT, cardChip, integratedText, metaFact } from "./chip";

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
  review_claim: null,
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
          { kind: "human_review", closable: false, action: null },
          { kind: "question", closable: true, action: null },
        ],
        blocker: BLOCKER,
      }),
    ).toBe("Question for you");
    expect(
      chipText({
        attention: [{ kind: "human_review", closable: false, action: null }],
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
        attention: [{ kind: "stalled", closable: false, action: null }],
      }),
    );
    expect(chip?.text).toBe("Blocked by T-9");
    expect(chip?.tone).toBe("bad");
  });

  it("shows a stall, then a stale claim, in gold", () => {
    const stalled = cardChip(
      boardCard("T-1", {
        attention: [
          { kind: "stale_claim", closable: false, action: null },
          { kind: "stalled", closable: false, action: null },
        ],
      }),
    );
    expect(stalled?.text).toBe("Stalled · needs you");
    expect(stalled?.tone).toBe("warn");
    expect(
      chipText({
        attention: [{ kind: "stale_claim", closable: false, action: null }],
      }),
    ).toBe("Stale claim · needs you");
  });

  it("shows a check run that goes in the neutral tone, before a stall and a stale claim and after a blocker", () => {
    const run = {
      run: "cr-4",
      purpose: "check" as const,
      proposal: null,
      started_at: 0,
    };
    const card = boardCard("T-1", {
      status: "in_progress",
      attention: [{ kind: "stalled", closable: false, action: null }],
      proposal: proposal("changes_requested"),
    });
    const chip = cardChip(card, run);
    expect(chip?.text).toBe("Running checks");
    expect(chip?.tone).toBe("neutral");
    expect(cardChip(card)?.text).toBe("Stalled · needs you");
    expect(
      cardChip(
        { ...card, blocker: { reason: "keys", set_by: "ana", set_at: 0 } },
        run,
      )?.text,
    ).toBe("Blocker");
  });

  it("never shows a stale claim over a check run that goes", () => {
    const run = {
      run: "cr-4",
      purpose: "check" as const,
      proposal: null,
      started_at: 0,
    };
    const card = boardCard("T-1", {
      status: "in_progress",
      attention: [{ kind: "stale_claim", closable: false, action: null }],
    });
    expect(cardChip(card, run)?.text).toBe("Running checks");
    expect(cardChip(card)?.text).toBe("Stale claim · needs you");
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

  it("says an open proposal is being reviewed while a reviewer holds its claim, in a review plan too", () => {
    const held = { review_claim: "review-2" };
    const chip = cardChip(
      boardCard("T-1", { proposal: proposal("open", held) }),
    );
    expect(chip?.text).toBe("P-3 being reviewed");
    expect(chip?.tone).toBe("neutral");
    expect(
      chipText({
        proposal: proposal("open", {
          ...held,
          review_plan: {
            plan: "rp-1",
            awaited: [],
            deadline_at: 10,
            wait_ended: true,
            proposals: ["P-3", "P-4"],
          },
        }),
      }),
    ).toBe("P-3 being reviewed");
    expect(chipText({ proposal: proposal("changes_requested", held) })).toBe(
      "P-3 changes requested",
    );
  });

  it("gives an approved proposal, a quiet card and a Done card no chip", () => {
    expect(chipText({ proposal: proposal("approved") })).toBeNull();
    expect(chipText({})).toBeNull();
    expect(
      cardChip(
        boardCard("T-1", {
          status: "done",
          attention: [{ kind: "question", closable: true, action: null }],
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

  it("shows what fold saw of the worker: work in the neutral tone, a stall in gold only once it waits for a person", () => {
    const card = boardCard("T-1", { status: "in_progress" });
    const working = {
      state: "working" as const,
      text: "Running a command for 8 min",
      stall: false,
      needsYou: false,
      restart: null,
      told: [],
      since: 0,
      waitFor: null,
    };
    expect(cardChip(card, null, working)).toMatchObject({
      tone: "neutral",
      text: "Running a command for 8 min",
    });
    const silent = {
      ...working,
      state: "silent" as const,
      text: "No activity from @web-worker for 12 min",
      stall: true,
      restart: "web-worker",
      told: ["web-lead"],
    };
    expect(cardChip(card, null, silent)?.tone).toBe("neutral");
    expect(cardChip(card, null, { ...silent, needsYou: true })).toMatchObject({
      tone: "warn",
      text: "No activity from @web-worker for 12 min",
    });
    const stalled = boardCard("T-1", {
      status: "in_progress",
      attention: [{ kind: "stalled", closable: false, action: null }],
    });
    expect(cardChip(stalled, null, working)?.text).toBe(
      "Running a command for 8 min",
    );
    expect(cardChip(stalled)?.text).toBe("Stalled · needs you");
    const waits = {
      ...working,
      state: "waits_for_slot" as const,
      text: "Waiting for a free slot",
    };
    expect(cardChip(card, null, waits)).toEqual({
      tone: "neutral",
      icon: Hourglass,
      text: SLOT_CHIP_TEXT,
    });
    expect(cardChip(card, null, working)?.icon).toBe(Activity);
    const network = {
      ...working,
      state: "waits_for_network" as const,
      text: "Waiting for the network",
    };
    expect(cardChip(card, null, network)).toEqual({
      tone: "neutral",
      icon: CloudOff,
      text: "Waiting for the network",
    });
    const limit = {
      ...working,
      state: "waits_for_usage_limit" as const,
      text: "Waits for the usage limit reset",
    };
    expect(cardChip(card, null, limit)).toEqual({
      tone: "neutral",
      icon: Gauge,
      text: "Waits for the usage limit reset",
    });
  });

  it("shows a worker that waits for its lead's go in the neutral tone, never as a stall", () => {
    const card = boardCard("T-1", { status: "in_progress" });
    const parked = {
      state: "parked" as const,
      text: "Waits for @backend-lead's go",
      stall: false,
      needsYou: false,
      restart: null,
      told: [],
      since: 0,
      waitFor: "my go after the T-288 migration",
    };
    expect(cardChip(card, null, parked)).toEqual({
      tone: "neutral",
      icon: CirclePause,
      text: "Waits for @backend-lead's go",
    });
  });

  it("shows a stale claim that waits for a person over a neutral activity, never over a stall that waits for one", () => {
    const card = boardCard("T-1", {
      status: "in_progress",
      attention: [
        { kind: "stale_claim", closable: false, action: "message_worker" },
      ],
    });
    const parked = {
      state: "parked" as const,
      text: "Waits for @backend-lead's go",
      stall: false,
      needsYou: false,
      restart: null,
      told: [],
      since: 0,
      waitFor: null,
    };
    expect(cardChip(card, null, parked)).toEqual({
      tone: "warn",
      icon: Hand,
      text: "Stale claim · message worker",
    });
    const silent = {
      ...parked,
      state: "silent" as const,
      text: "No activity from @w for 40 min",
      stall: true,
      needsYou: true,
    };
    expect(cardChip(card, null, silent)?.text).toBe(
      "No activity from @w for 40 min",
    );
  });

  it("names the person's action on the gold chip of a stall or a stale claim that only the daemon's item tells of", () => {
    const stale = boardCard("T-1", {
      status: "in_progress",
      attention: [
        { kind: "stale_claim", closable: false, action: "message_worker" },
      ],
    });
    expect(cardChip(stale)).toEqual({
      tone: "warn",
      icon: Hand,
      text: "Stale claim · message worker",
    });
    const stalled = boardCard("T-1", {
      status: "in_progress",
      attention: [
        { kind: "stalled", closable: false, action: "restart_worker" },
      ],
    });
    expect(cardChip(stalled)?.text).toBe("Stalled · restart worker");
  });

  it("shows a worker stopped on purpose in the neutral tone with the stop icon, and a failed one as a stall", () => {
    const card = boardCard("T-1", { status: "in_progress" });
    const stopped = {
      state: "stopped" as const,
      text: "@web-worker stopped",
      stall: false,
      needsYou: false,
      restart: null,
      told: [],
      since: 0,
      waitFor: null,
    };
    expect(cardChip(card, null, stopped)).toEqual({
      tone: "neutral",
      icon: CircleStop,
      text: "@web-worker stopped",
    });
    const failed = {
      ...stopped,
      state: "failed" as const,
      text: "@web-worker failed",
      stall: true,
    };
    expect(cardChip(card, null, failed)).toEqual({
      tone: "neutral",
      icon: Clock,
      text: "@web-worker failed",
    });
    expect(cardChip(card, null, { ...failed, needsYou: true })).toEqual({
      tone: "warn",
      icon: Hand,
      text: "@web-worker failed",
    });
  });
});
