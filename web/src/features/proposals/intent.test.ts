import { describe, expect, it } from "vitest";
import {
  proposalDetail,
  proposalSummary,
  proposalVersion,
  review,
} from "@/test/proposals";
import { historyEntries } from "./history";
import {
  NO_INTENT,
  intentCard,
  reviewerSummary,
  versionIntent,
} from "./intent";

function entries(
  versions: ReturnType<typeof proposalVersion>[],
  reviews: ReturnType<typeof review>[] = [],
) {
  const p = proposalDetail(
    proposalSummary("P-4", { current_version: versions.length }),
    { versions, reviews },
  );
  return historyEntries(p, new Map());
}

describe("the intent card", () => {
  it("shows the current version's intent and who wrote it", () => {
    const card = intentCard(
      entries([
        proposalVersion(1, { intent: "Adds the table." }),
        proposalVersion(2, {
          intent: "  Adds the table and its index.\n",
          created_by: "review/quality",
          reviewer_fix: true,
        }),
      ]),
    );
    expect(card).toMatchObject({
      version: 2,
      intent: "Adds the table and its index.",
      by: "review/quality",
      origin: "review_fix",
      summary: null,
    });
  });

  it("has no intent for a version made before intents, and no card without a version", () => {
    expect(intentCard(entries([proposalVersion(1)]))?.intent).toBeNull();
    expect(intentCard([])).toBeNull();
    expect(NO_INTENT).toContain("no intent");
  });

  it("takes the newest verdict as the reviewer's summary, and a comment only without one", () => {
    const withVerdict = intentCard(
      entries(
        [proposalVersion(1)],
        [
          review(1, "review/a", "approve", {
            body: "Matches the intent.",
            created_at: 1,
          }),
          review(1, "review/a", "comment", { body: "a note", created_at: 2 }),
        ],
      ),
    );
    expect(withVerdict?.summary?.review.verdict).toBe("approve");
    expect(withVerdict?.summary?.excerpt.text).toBe("Matches the intent.");
    const onlyComment = reviewerSummary(
      entries(
        [proposalVersion(1)],
        [review(1, "review/a", "comment", { body: "a note" })],
      )[0]?.reviews ?? [],
    );
    expect(onlyComment?.review.verdict).toBe("comment");
    expect(reviewerSummary([])).toBeNull();
  });

  it("reads an empty intent as none", () => {
    expect(versionIntent({ intent: " \n" })).toBeNull();
    expect(versionIntent({ intent: null })).toBeNull();
    expect(versionIntent({ intent: " a " })).toBe("a");
  });
});
