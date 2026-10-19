import { bareName, isHumanName } from "@/people/names";
import { describe, expect, it } from "vitest";
import type { Review } from "@/api/proposals";
import {
  proposalDetail,
  proposalSummary,
  proposalVersion,
  review as reviewOf,
} from "@/test/proposals";
import {
  approvers,
  claimLine,
  claimText,
  currentVersion,
  evidenceLines,
  evidenceSource,
  fileCount,
  filesText,
  listedReviews,
  reviewPill,
  reviewScopeText,
  reviewText,
  versionAuthor,
  versionRef,
  versionText,
  versionVerdict,
  waitingOn,
} from "./present";

function review(
  version: number,
  verdict: Review["verdict"],
  body = "",
  at = 0,
  reviewer = "review/quality",
): Review {
  return reviewOf(version, reviewer, verdict, { body, created_at: at });
}

const mark = { reviewer: "tanim", reason: "schema work", set_by: "backend" };

describe("approvers", () => {
  it("lists who approved the version last, in approval order", () => {
    expect(
      approvers(2, [
        review(2, "approve", "", 1, "review/quality"),
        review(2, "approve", "", 2, "human:tanim"),
        review(2, "request_changes", "", 3, "review/quality"),
        review(1, "approve", "", 4, "review/security"),
        review(2, "approve", "", 5, "review/security"),
      ]),
    ).toEqual(["human:tanim", "review/security"]);
  });
});

describe("reviewPill", () => {
  const p4 = proposalSummary("P-4", {
    current_version: 2,
    human_review: mark,
    latest_verdict: "approve",
    human_approved: false,
  });

  it("states the review state for the list and the header", () => {
    expect(reviewPill(p4)).toEqual({
      tone: "human",
      icon: "human",
      text: "Agent approved · needs @tanim",
      segments: ["Agent approved · needs ", { actor: "human:tanim" }],
    });
    expect(reviewPill(p4, { long: true }).text).toBe(
      "Agent approved · waiting for human review",
    );
    const p3 = proposalSummary("P-3", { turn: "with_author" });
    expect(reviewPill(p3)).toEqual({
      tone: "danger",
      icon: "changes",
      text: "Changes requested",
    });
    expect(reviewPill(p3, { long: true }).text).toBe(
      "Changes requested · back with the author",
    );
    const p6 = proposalSummary("P-6", {
      conflicted: true,
      turn: "with_author",
    });
    expect(reviewPill(p6)).toEqual({
      tone: "danger",
      icon: "conflict",
      text: "Conflict",
    });
    expect(reviewPill(p6, { long: true }).text).toBe(
      "Conflict · back with the author",
    );
    expect(reviewPill(p6, { long: true, conflictMain: "r9" }).text).toBe(
      "Conflict with main r9 · back with the author",
    );
    expect(reviewPill(p6, { conflictMain: "r9" }).text).toBe("Conflict");
    const p5 = proposalSummary("P-5", { state: "integrated", turn: null });
    expect(reviewPill(p5, { revision: "r8" }).text).toBe("Integrated as r8");
    expect(reviewPill(p5).text).toBe("Integrated");
    expect(
      reviewPill(proposalSummary("P-8", { state: "closed", turn: null })).text,
    ).toBe("Closed");
  });

  it("tells an approval with the human approval from one without it", () => {
    expect(reviewPill({ ...p4, human_approved: true })).toEqual({
      tone: "approved",
      icon: "approved",
      text: "Approved",
    });
    expect(
      reviewPill({ ...p4, human_review: { ...mark, reviewer: null } }).text,
    ).toBe("Agent approved · needs a human");
    expect(
      reviewPill({ ...p4, human_review: null, human_approved: true }).text,
    ).toBe("Approved");
    expect(reviewPill({ ...p4, latest_verdict: "request_changes" }).text).toBe(
      "Changes requested",
    );
    expect(reviewPill({ ...p4, latest_verdict: null }).text).toBe(
      "Waiting for review",
    );
  });
});

describe("names and versions", () => {
  it("shows a human by name and a worker in full", () => {
    expect(isHumanName("human:tanim")).toBe(true);
    expect(bareName("human:tanim")).toBe("tanim");
    expect(bareName("backend/db")).toBe("backend/db");
  });

  it("says who the proposal waits on from the turn", () => {
    expect(
      waitingOn(proposalSummary("P-4", { review_claim: "review/quality" })),
    ).toEqual({ actor: "review/quality" });
    expect(waitingOn(proposalSummary("P-4"))).toBeNull();
    const needsHuman = proposalSummary("P-4", {
      review_claim: "review/quality",
      latest_verdict: "approve",
      human_review: mark,
    });
    expect(waitingOn(needsHuman)).toEqual({ actor: "human:tanim" });
    expect(
      waitingOn({
        ...needsHuman,
        human_review: { ...mark, reviewer: null },
      }),
    ).toEqual({ actor: null });
    expect(waitingOn({ ...needsHuman, human_approved: true })).toEqual({
      actor: "review/quality",
    });
    expect(
      waitingOn(
        proposalSummary("P-3", {
          turn: "with_author",
          author: "frontend/styles",
        }),
      ),
    ).toEqual({ actor: "frontend/styles" });
    expect(
      waitingOn(proposalSummary("P-5", { state: "integrated", turn: null })),
    ).toBeNull();
  });

  it("names versions and the latest verdict of each", () => {
    const reviews = [
      review(1, "request_changes", "", 1),
      review(2, "approve", "", 2),
    ];
    expect(versionText(proposalSummary("P-4", { current_version: 2 }))).toBe(
      "v2 of 2",
    );
    expect(versionRef("P-4", 1)).toBe("P-4 v1");
    expect(versionVerdict(1, reviews)).toBe("changes requested");
    expect(versionVerdict(2, reviews)).toBe("approved");
    expect(versionVerdict(1, [review(1, "comment")])).toBe("no verdict yet");
    expect(versionVerdict(1, [review(1, "reject")])).toBe("rejected");
    expect(
      versionVerdict(1, [
        review(1, "approve", "", 5),
        review(1, "request_changes", "", 2),
      ]),
    ).toBe("approved");
  });

  it("describes the current version's files with the payload's totals, and the claim", () => {
    const p = proposalDetail(proposalSummary("P-4", { current_version: 2 }), {
      versions: [
        proposalVersion(1),
        proposalVersion(2, { files_added: 2, files_modified: 1 }),
      ],
      totals: { files: 3, added_lines: 96, removed_lines: 4 },
    });
    const current = currentVersion(p);
    expect(current?.version).toBe(2);
    expect(current ? filesText(current, p.totals) : "").toBe(
      "3 files · +96 −4",
    );
    expect(currentVersion({ ...p, versions: [] })).toBeNull();
    const one = proposalVersion(1, { files_modified: 0 });
    expect(fileCount(one)).toBe(1);
    expect(filesText(one, { files: 1, added_lines: 1, removed_lines: 0 })).toBe(
      "1 file · +1 −0",
    );
    expect(
      claimText(proposalSummary("P-4", { review_claim: "review/quality" })),
    ).toEqual(["review claimed by ", { actor: "review/quality" }]);
    expect(claimText(proposalSummary("P-3"))).toEqual(["review not claimed"]);
    expect(claimLine(proposalSummary("P-3"))).toEqual(["unclaimed"]);
    expect(
      claimLine(proposalSummary("P-4", { review_claim: "review/quality" })),
    ).toEqual(["claimed by ", { actor: "review/quality" }]);
  });
});

describe("review scopes", () => {
  const scoped = (scope: NonNullable<Review["scope"]>): Review => ({
    ...review(3, "approve"),
    scope,
  });

  it("names the scope a review of a revised version followed, the rules behind a suggested full review and a chosen scope's reason", () => {
    expect(reviewScopeText(review(1, "approve"))).toBeNull();
    expect(
      reviewScopeText(
        scoped({
          scope: "change",
          since: 2,
          suggested: "change",
          rules: [],
          reason: null,
        }),
      ),
    ).toBe("Change review since v2");
    expect(
      reviewScopeText(
        scoped({
          scope: "full",
          since: 1,
          suggested: "full",
          rules: ["new_files", "base_conflicts", "v3_or_later"],
          reason: null,
        }),
      ),
    ).toBe(
      "Full review since v1 · suggested for new files, files merged with main by hand, v3 or later",
    );
    expect(
      reviewScopeText(
        scoped({
          scope: "change",
          since: 2,
          suggested: "full",
          rules: ["v3_or_later", "large_delta"],
          reason: "only strings changed",
        }),
      ),
    ).toBe(
      "Change review since v2 · chosen: only strings changed · fold suggested a full review for v3 or later, a large change",
    );
    expect(
      reviewScopeText(
        scoped({
          scope: "full",
          since: 2,
          suggested: "change",
          rules: [],
          reason: "a public API",
        }),
      ),
    ).toBe(
      "Full review since v2 · chosen: a public API · fold suggested a change review",
    );
    expect(
      reviewScopeText(
        scoped({
          scope: "change",
          since: 2,
          suggested: "change",
          rules: [],
          reason: "confirmed",
        }),
      ),
    ).toBe("Change review since v2 · confirmed");
  });

  it("gives no scope line for a human's review of a revised version", () => {
    expect(
      reviewScopeText(review(3, "approve", "", 0, "human:tanim")),
    ).toBeNull();
    expect(
      reviewScopeText({
        ...review(3, "approve", "", 0, "human:tanim"),
        scope: {
          scope: "full",
          since: 2,
          suggested: "full",
          rules: ["v3_or_later"],
          reason: null,
        },
      }),
    ).toBeNull();
  });
});

describe("reviews and evidence", () => {
  it("writes a verdict with its body, and lists reviews newest first without the bodiless comment reviews of line comments", () => {
    expect(reviewText(review(1, "request_changes", "Missing index"))).toBe(
      "Requested changes: Missing index",
    );
    expect(reviewText(review(2, "approve", "  "))).toBe("Approved");
    expect(reviewText({ ...review(2, "approve"), kept_from: 1 })).toBe(
      "Approved (kept from v1)",
    );
    expect(reviewText(review(1, "comment", "Naming matches T-14"))).toBe(
      "Comment: Naming matches T-14",
    );
    expect(reviewText(review(1, "reject", "Out of scope"))).toBe(
      "Rejected: Out of scope",
    );
    expect(
      listedReviews([
        review(1, "comment", "Naming matches T-14", 1),
        review(1, "comment", " ", 2),
        review(2, "approve", "", 3),
      ]).map((r) => r.body),
    ).toEqual(["", "Naming matches T-14"]);
  });

  it("splits evidence into commands and their output", () => {
    expect(evidenceLines("$ cargo test\n  ok")).toEqual([
      { text: "$ cargo test", command: true },
      { text: "  ok", command: false },
    ]);
  });
});

describe("who made a version", () => {
  it("names the author's worker, the reviewer of a fix or the human of an edit", () => {
    expect(versionAuthor(proposalVersion(1))).toBe("backend/db");
    expect(
      versionAuthor(
        proposalVersion(2, { created_by: "review", reviewer_fix: true }),
      ),
    ).toBe("review");
    expect(
      versionAuthor(
        proposalVersion(3, { created_by: "human:tanim", human_edit: true }),
      ),
    ).toBe("tanim");
  });

  it("gives a reviewer's fix the reviewer's evidence", () => {
    expect(evidenceSource(proposalVersion(1))).toBe("the author");
    expect(evidenceSource(proposalVersion(2, { reviewer_fix: true }))).toBe(
      "the reviewer",
    );
    expect(evidenceSource(undefined)).toBe("the author");
  });
});
