import { describe, expect, it } from "vitest";
import { AGENT_APPROVES_FIRST, CHECKS_FAILED } from "./actions";
import { approveReason, barNotes, phoneMore, reviewerLine } from "./decisions";

const ALL = ["approve", "request_changes", "integrate"] as const;

describe("reviewerLine", () => {
  it("names the version and the human", () => {
    expect(reviewerLine(2, "dev")).toBe("You review v2 as dev");
  });
});

describe("barNotes", () => {
  it("says nothing while every button works", () => {
    expect(barNotes(ALL, null, null, null)).toEqual([]);
  });

  it("says a reason both buttons share once, and Approve points to it", () => {
    const notes = barNotes(ALL, CHECKS_FAILED, CHECKS_FAILED, null);
    expect(notes).toEqual([{ kind: "integrate", text: CHECKS_FAILED }]);
    expect(approveReason(notes, CHECKS_FAILED)).toBe("integrate");
  });

  it("keeps two different reasons in the order of the buttons, then the kept approvals", () => {
    const notes = barNotes(
      [...ALL, "review_again"],
      "You approved v2.",
      AGENT_APPROVES_FIRST,
      "@review keeps its approval of v1.",
    );
    expect(notes.map((note) => note.kind)).toEqual([
      "approve",
      "integrate",
      "kept",
    ]);
    expect(approveReason(notes, "You approved v2.")).toBe("approve");
  });

  it("leaves out the line of a button that does not show", () => {
    expect(
      barNotes(["request_changes"], CHECKS_FAILED, CHECKS_FAILED, null),
    ).toEqual([]);
    expect(barNotes(["approve"], CHECKS_FAILED, CHECKS_FAILED, null)).toEqual([
      { kind: "approve", text: CHECKS_FAILED },
    ]);
  });

  it("names no reason for an Approve that works", () => {
    expect(approveReason([], null)).toBeNull();
  });
});

describe("phoneMore", () => {
  it("puts Integrate and Send for review again under More", () => {
    expect(phoneMore([...ALL, "review_again"])).toEqual([
      "integrate",
      "review_again",
    ]);
    expect(phoneMore(["approve"])).toEqual([]);
  });
});
