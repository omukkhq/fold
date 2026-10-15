import { describe, expect, it } from "vitest";
import type { ProposalDetail, Review } from "@/api/proposals";
import { detail } from "../../../fixtures/proposals";
import {
  claimText,
  currentVersion,
  displayName,
  evidenceLines,
  filesText,
  isHuman,
  listedReviews,
  reviewPill,
  reviewText,
  versionRef,
  versionText,
  versionVerdict,
  waitingOn,
} from "./present";

function proposal(
  id: string,
  fields: Partial<ProposalDetail> = {},
): ProposalDetail {
  const found = detail("demo", id);
  if (!found) throw new Error(`no fixture ${id}`);
  return { ...structuredClone(found), ...fields };
}

function review(
  version: number,
  verdict: Review["verdict"],
  body = "",
  at = 0,
): Review {
  return { version, reviewer: "review/quality", verdict, body, created_at: at };
}

describe("reviewPill", () => {
  it("states the review state for the list and the header", () => {
    expect(reviewPill(proposal("P-4")).text).toBe(
      "Agent approved · needs tanim",
    );
    expect(reviewPill(proposal("P-4"), true).text).toBe(
      "Agent approved · waiting for human review",
    );
    expect(reviewPill(proposal("P-3"))).toEqual({
      tone: "danger",
      icon: "changes",
      text: "Changes requested",
    });
    expect(reviewPill(proposal("P-3"), true).text).toBe(
      "Changes requested · back with the author",
    );
    expect(reviewPill(proposal("P-6"))).toEqual({
      tone: "danger",
      icon: "conflict",
      text: "Conflict",
    });
    expect(reviewPill(proposal("P-6"), true).text).toBe(
      "Conflict · back with the author",
    );
    expect(reviewPill(proposal("P-6"), true, "r9").text).toBe(
      "Conflict with main r9 · back with the author",
    );
    expect(reviewPill(proposal("P-6"), false, "r9").text).toBe("Conflict");
    expect(reviewPill(proposal("P-5")).text).toBe("Integrated as r8");
    expect(
      reviewPill(proposal("P-5", { integrated_revision: null })).text,
    ).toBe("Integrated as main");
    expect(reviewPill(proposal("P-8")).text).toBe("Closed");
  });

  it("names any human when the mark names nobody, and says approved once a human approved", () => {
    const p = proposal("P-4");
    expect(
      reviewPill({
        ...p,
        human_review: p.human_review && { ...p.human_review, reviewer: null },
      }).text,
    ).toBe("Agent approved · needs a human");
    expect(reviewPill({ ...p, human_approved: true }).text).toBe("Approved");
    expect(reviewPill({ ...p, latest_verdict: null }).text).toBe(
      "Waiting for review",
    );
  });
});

describe("names and versions", () => {
  it("shows a human by name and a worker in full", () => {
    expect(isHuman("human:tanim")).toBe(true);
    expect(displayName("human:tanim")).toBe("tanim");
    expect(displayName("backend/db")).toBe("backend/db");
  });

  it("says who the proposal waits on", () => {
    expect(waitingOn(proposal("P-4"))).toEqual({ name: "tanim", human: true });
    expect(waitingOn(proposal("P-3"))).toEqual({
      name: "frontend/styles",
      human: false,
    });
    expect(waitingOn(proposal("P-5"))).toBeNull();
  });

  it("names versions and the latest verdict of each", () => {
    const p = proposal("P-4");
    expect(versionText(p)).toBe("v2 of 2");
    expect(versionRef("P-4", 1)).toBe("P-4 v1");
    expect(versionVerdict(1, p.reviews)).toBe("changes requested");
    expect(versionVerdict(2, p.reviews)).toBe("approved");
    expect(versionVerdict(1, [review(1, "comment")])).toBe("no verdict yet");
    expect(versionVerdict(1, [review(1, "reject")])).toBe("rejected");
    expect(
      versionVerdict(1, [
        review(1, "approve", "", 5),
        review(1, "request_changes", "", 2),
      ]),
    ).toBe("approved");
  });

  it("describes the current version's files and the claim", () => {
    const p = proposal("P-4");
    const current = currentVersion(p);
    expect(current?.version).toBe(2);
    expect(current ? filesText(current) : "").toMatch(/^3 files · \+\d+ −2$/);
    expect(currentVersion({ ...p, versions: [] })).toBeNull();
    expect(claimText(p)).toBe("review claimed by review/quality");
    expect(claimText(proposal("P-3"))).toBe("review not claimed");
    const one = proposal("P-5").versions[0];
    expect(one ? filesText(one) : "").toBe("1 file · +1 −0");
  });
});

describe("reviews and evidence", () => {
  it("writes a verdict with its body, and lists reviews newest first without the bodiless comment reviews of line comments", () => {
    expect(reviewText(review(1, "request_changes", "Missing index"))).toBe(
      "Requested changes: Missing index",
    );
    expect(reviewText(review(2, "approve", "  "))).toBe("Approved");
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
