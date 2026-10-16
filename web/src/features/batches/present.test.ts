import { describe, expect, it } from "vitest";
import { batch, member, proposalSummary, workerProcess } from "@/test/workers";
import {
  checkoutView,
  conflictedMembers,
  memberCountText,
  memberRows,
  reviewLead,
  reviewerLine,
  workerTotals,
} from "./present";

const mark = { reason: "auth", reviewer: "tanim", set_by: "main" };

describe("memberRows", () => {
  it("orders members by merge position and joins the proposal list", () => {
    const b = batch({
      members: [
        member("P-4", { position: 2, task: "T-11" }),
        member("P-3", { position: 1, version: 2, current_version: 2 }),
      ],
      verdicts: [{ proposal: "P-3", verdict: "approve", version: 2 }],
    });
    const rows = memberRows(b, [
      proposalSummary("P-3", { human_review: mark, human_approved: false }),
      proposalSummary("P-4", { author: "web-styles" }),
    ]);
    expect(rows).toEqual([
      {
        proposal: "P-3",
        title: "Proposal P-3",
        task: "T-12",
        author: "web-coder",
        version: "v2 of 2",
        verdict: "approved",
        needsHuman: "human:tanim",
      },
      {
        proposal: "P-4",
        title: "Proposal P-4",
        task: "T-11",
        author: "web-styles",
        version: "v1 of 1",
        verdict: "reviewing",
        needsHuman: null,
      },
    ]);
  });

  it("tells conflicts, members that left, closed batches and unknown proposals apart", () => {
    const open = batch({
      members: [
        member("P-5", { position: 1 }),
        member("P-6", { position: 2, in_batch: false }),
        member("P-7", { position: 3 }),
      ],
      verdicts: [{ proposal: "P-7", verdict: "request_changes", version: 2 }],
    });
    const rows = memberRows(open, [
      proposalSummary("P-5", { conflicted: true }),
      proposalSummary("P-6", {
        human_review: { ...mark, reviewer: null },
        human_approved: false,
      }),
    ]);
    expect(rows.map((r) => r.verdict)).toEqual([
      "conflict",
      "left",
      "reviewing",
    ]);
    expect(rows[1]?.needsHuman).toBe("");
    expect(rows[2]).toMatchObject({ title: null, author: null });
    expect(conflictedMembers(rows)).toEqual(["P-5"]);

    const closed = memberRows(
      batch({
        state: "submitted",
        members: [member("P-8", { in_batch: false })],
        verdicts: [{ proposal: "P-9", verdict: "reject", version: 1 }],
      }),
      [],
    );
    expect(closed[0]?.verdict).toBe("none");
    expect(
      memberRows(
        batch({
          members: [member("P-9")],
          verdicts: [{ proposal: "P-9", verdict: "comment", version: 1 }],
        }),
        [],
      )[0]?.verdict,
    ).toBe("commented");
  });
});

describe("batch texts", () => {
  it("counts members still in an open batch, and all of a finished one", () => {
    const members = [member("P-3"), member("P-4", { in_batch: false })];
    expect(memberCountText(batch({ members }))).toBe(
      "1 proposal, one verdict each",
    );
    expect(memberCountText(batch({ state: "closed", members }))).toBe(
      "2 proposals, one verdict each",
    );
  });

  it("words the checkout per batch state", () => {
    const checkout = {
      base: "r17",
      path: ".fold/ws/review-rb-3",
      workspace: "ws-12",
    };
    const members = [member("P-3"), member("P-4")];
    expect(checkoutView(batch({ checkout, members }))).toEqual({
      label: "ready",
      tone: "ready",
      line: "ws-12 · main r17 + P-3 + P-4",
      path: ".fold/ws/review-rb-3",
    });
    expect(checkoutView(batch())).toEqual({
      label: "no checkout",
      tone: "starting",
      line: null,
      path: null,
    });
    expect(checkoutView(batch({ state: "submitted" })).label).toBe("submitted");
    expect(checkoutView(batch({ state: "closed" })).label).toBe("closed");
  });

  it("sums up a worker and finds the review lead", () => {
    expect(workerTotals(workerProcess("r", { turns: 21 }))).toBe(
      "21 turns · $5.87 · context 164k",
    );
    expect(workerTotals(workerProcess("r", { turns: 1 }))).toContain(
      "1 turn ·",
    );
    expect(
      reviewLead([
        workerProcess("old", { role: "review_lead" }),
        workerProcess("web-lead"),
        workerProcess("review", { role: "review_lead" }),
      ]),
    ).toBe("review");
    expect(reviewLead([])).toBeNull();
  });

  it("names the reviewer's role and the model it has", () => {
    const reviewer = workerProcess("r", {
      role: "batch_reviewer",
      model: "sonnet",
    });
    expect(reviewerLine(reviewer)).toBe("Batch reviewer · sonnet");
    expect(reviewerLine({ ...reviewer, model: null })).toBe("Batch reviewer");
  });
});
