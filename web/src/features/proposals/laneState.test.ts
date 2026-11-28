import { describe, expect, it } from "vitest";
import type { ConflictedReason } from "@/api/proposals";
import { segmentsText } from "@/people/segments";
import { proposalSummary } from "@/test/proposals";
import {
  fixerSegments,
  isMergeConflict,
  laneFailure,
  laneFailureLine,
  laneFailureText,
  laneRefresh,
  laneRefreshLine,
  laneRefreshText,
  laneText,
  refresherSegments,
} from "./laneState";

const failed: ConflictedReason = {
  kind: "lane_checks_failed",
  lane: "migrations",
  run: "cr-4",
  fixer: "reviewer",
};
const merge: ConflictedReason = { kind: "merge_conflict", fixer: "reviewer" };
const refresh: ConflictedReason = {
  kind: "lane_refresh",
  lane: "migrations",
  refresh: "put the migration on the current head",
  fixer: "reviewer",
};

describe("laneRefresh", () => {
  it("returns only a lane's refresh", () => {
    expect(laneRefresh(refresh)).toBe(refresh);
    expect(laneRefresh(failed)).toBeNull();
    expect(laneRefresh(merge)).toBeNull();
    expect(laneRefresh(null)).toBeNull();
  });

  it("is not a merge conflict", () => {
    const flagged = proposalSummary("P-1", { conflicted: true });
    expect(isMergeConflict({ ...flagged, conflicted_reason: refresh })).toBe(
      false,
    );
  });

  it("names the lane, its text and who redoes it", () => {
    const found = laneRefresh(refresh);
    if (found === null) throw new Error("no refresh");
    expect(laneRefreshText(found)).toBe(
      "Lane migrations asks for a refresh: put the migration on the current head",
    );
    expect(segmentsText(laneRefreshLine(found))).toBe(
      "Lane migrations asks for a refresh: put the migration on the current head · @reviewer redoes it",
    );
    expect(segmentsText(refresherSegments({ ...found, fixer: null }))).toBe(
      "a reviewer redoes it",
    );
  });
});

describe("laneFailure", () => {
  it("returns only a lane check failure", () => {
    expect(laneFailure(failed)).toBe(failed);
    expect(laneFailure(merge)).toBeNull();
    expect(laneFailure(null)).toBeNull();
    expect(laneFailure(undefined)).toBeNull();
  });
});

describe("isMergeConflict", () => {
  it("is a merge conflict for the merge reason and for a flag without a reason", () => {
    const flagged = proposalSummary("P-1", { conflicted: true });
    expect(isMergeConflict(flagged)).toBe(true);
    expect(isMergeConflict({ ...flagged, conflicted_reason: merge })).toBe(
      true,
    );
  });

  it("is not a merge conflict for a lane check failure or an unflagged proposal", () => {
    const flagged = proposalSummary("P-1", { conflicted: true });
    expect(isMergeConflict({ ...flagged, conflicted_reason: failed })).toBe(
      false,
    );
    expect(isMergeConflict(proposalSummary("P-2"))).toBe(false);
  });
});

describe("lane failure wording", () => {
  it("names the lane and the fixer", () => {
    expect(laneFailureText(failed)).toBe(
      "Integration checks failed after lane migrations moved",
    );
    expect(segmentsText(fixerSegments(failed))).toBe("@reviewer fixes it");
    expect(segmentsText(laneFailureLine(failed))).toBe(
      "Integration checks failed after lane migrations moved · @reviewer fixes it",
    );
  });

  it("says a reviewer fixes it while no fixer is named", () => {
    expect(segmentsText(fixerSegments({ ...failed, fixer: null }))).toBe(
      "a reviewer fixes it",
    );
  });
});

describe("laneText", () => {
  it("gives the lane and the place", () => {
    expect(
      laneText({
        name: "migrations",
        position: 2,
        held: "lane migrations, position 2",
      }),
    ).toBe("Lane migrations, position 2");
    expect(laneText({ name: "migrations", position: null, held: null })).toBe(
      "Lane migrations",
    );
  });

  it("is null outside every lane", () => {
    expect(laneText(null)).toBeNull();
    expect(laneText(undefined)).toBeNull();
  });
});
