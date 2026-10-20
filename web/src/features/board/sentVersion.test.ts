import { describe, expect, it } from "vitest";
import {
  type SentProposal,
  type SentWait,
  sentText,
  sentVersion,
  showsStall,
  waitText,
} from "./sentVersion";

function proposal(over: Partial<SentProposal> = {}): SentProposal {
  return {
    id: "P-1",
    state: "open",
    version: 1,
    checks_failed: false,
    ...over,
  };
}

function frame(proposal: string | null, edits: boolean | null) {
  return { proposal, edits_after_version: edits };
}

describe("sentVersion", () => {
  it("is the open version while the payload says nothing changed after it", () => {
    expect(sentVersion(proposal({ version: 2 }), false, null, [])).toEqual({
      proposal: "P-1",
      version: 2,
      waits: "review",
    });
  });

  it("is null without an open proposal, after an edit and before a comparison", () => {
    expect(sentVersion(null, false, null, [])).toBeNull();
    expect(
      sentVersion(proposal({ state: "integrated" }), false, null, []),
    ).toBeNull();
    expect(sentVersion(proposal(), true, null, [])).toBeNull();
    expect(sentVersion(proposal(), null, null, [])).toBeNull();
  });

  it("takes the answer of a live frame of the same proposal over the payload", () => {
    expect(sentVersion(proposal(), false, frame("P-1", true), [])).toBeNull();
    expect(sentVersion(proposal(), true, frame("P-1", false), [])).toEqual(
      expect.objectContaining({ waits: "review" }),
    );
    expect(
      sentVersion(proposal(), false, frame("P-1", null), [])?.version,
    ).toBe(1);
  });

  it("keeps the payload when the frame names another proposal or none", () => {
    expect(
      sentVersion(proposal(), false, frame("P-2", true), []),
    ).not.toBeNull();
    expect(sentVersion(proposal(), true, frame(null, false), [])).toBeNull();
  });

  const waits: [string, Partial<SentProposal>, { kind: string }[], SentWait][] =
    [
      [
        "a conflict first",
        { state: "conflict", checks_failed: true },
        [],
        "conflict",
      ],
      [
        "failed checks",
        { state: "approved", checks_failed: true },
        [{ kind: "human_review" }],
        "checks_failed",
      ],
      [
        "changes requested",
        { state: "changes_requested" },
        [{ kind: "human_review" }],
        "changes_requested",
      ],
      [
        "a human review the daemon asks for",
        { state: "approved" },
        [{ kind: "human_review" }],
        "human_review",
      ],
      [
        "the integration of an approved version",
        { state: "approved" },
        [],
        "integration",
      ],
      ["a review otherwise", {}, [{ kind: "stalled" }], "review"],
    ];
  for (const [name, over, attention, wait] of waits) {
    it(`waits for ${name}`, () => {
      expect(sentVersion(proposal(over), false, null, attention)?.waits).toBe(
        wait,
      );
    });
  }
});

describe("showsStall", () => {
  const stalled = [{ kind: "stalled" }];
  const sent = (waits: SentWait) => ({ proposal: "P-1", version: 1, waits });
  it("follows the daemon without a sent version", () => {
    expect(showsStall(null, stalled)).toBe(true);
    expect(showsStall(null, [])).toBe(false);
  });
  it("shows only while the sent version is back with its author", () => {
    const shown = (
      [
        "conflict",
        "checks_failed",
        "changes_requested",
        "human_review",
        "integration",
        "review",
      ] as const
    ).map((wait) => showsStall(sent(wait), stalled));
    expect(shown).toEqual([true, true, true, false, false, false]);
    expect(showsStall(sent("changes_requested"), [])).toBe(false);
  });
});

describe("the sent texts", () => {
  it("name the proposal and version, and each wait", () => {
    expect(sentText({ proposal: "P-1", version: 3, waits: "review" })).toBe(
      "sent P-1 v3",
    );
    expect(
      (
        [
          "conflict",
          "checks_failed",
          "changes_requested",
          "human_review",
          "integration",
          "review",
        ] as const
      ).map(waitText),
    ).toEqual([
      "conflicts with main",
      "integration checks failed",
      "changes requested",
      "waits for human review",
      "approved, waits for integration",
      "waits for review",
    ]);
  });
});
