import { describe, expect, it } from "vitest";
import { taskView } from "@/test/payloads";
import { proposalStrip } from "./proposalStrip";

const proposal = {
  id: "P-7",
  title: "Add the form",
  review_claim: null,
  files: 3,
  state: "open" as const,
  version: 2,
  checks_failed: false,
  reviews: [],
};

describe("proposalStrip", () => {
  it("is absent without a proposal and outside Review", () => {
    expect(proposalStrip(taskView("T-1", { status: "review" }))).toBeNull();
    expect(
      proposalStrip(
        taskView("T-1", {
          status: "in_progress",
          proposal: { ...proposal, state: "changes_requested" },
        }),
      ),
    ).toBeNull();
    expect(
      proposalStrip(
        taskView("T-1", {
          status: "in_progress",
          proposal: { ...proposal, checks_failed: true },
          attention: [{ kind: "human_review", closable: false }],
        }),
      ),
    ).toBeNull();
    expect(
      proposalStrip(
        taskView("T-1", {
          status: "done",
          proposal: { ...proposal, state: "integrated" },
        }),
      ),
    ).toBeNull();
  });

  it("says an approved version in Review is approved", () => {
    expect(
      proposalStrip(
        taskView("T-1", {
          status: "review",
          proposal: {
            ...proposal,
            state: "approved",
            review_claim: "review-lead",
          },
        }),
      ),
    ).toEqual({
      proposal: "P-7",
      lead: "P-7 v2 is approved",
      rest: "· Add the form · 3 files",
      waitsForYou: false,
    });
  });

  it("is gold and names the human while the version waits for human review", () => {
    expect(
      proposalStrip(
        taskView("T-1", {
          status: "review",
          proposal,
          attention: [{ kind: "human_review", closable: false }],
        }),
      ),
    ).toEqual({
      proposal: "P-7",
      lead: "P-7 v2 waits for your review",
      rest: "· Add the form · 3 files",
      waitsForYou: true,
    });
  });

  it("names the reviewer that holds the review claim and the title and files", () => {
    expect(
      proposalStrip(
        taskView("T-1", {
          status: "review",
          proposal: { ...proposal, review_claim: "review-lead" },
        }),
      ),
    ).toEqual({
      proposal: "P-7",
      lead: "P-7 v2 is with @review-lead",
      rest: "· Add the form · 3 files",
      waitsForYou: false,
    });
  });

  it("puts this human before the reviewer and leaves out what the read lacks", () => {
    expect(
      proposalStrip(
        taskView("T-1", {
          status: "review",
          proposal: {
            ...proposal,
            review_claim: "review-lead",
            title: "",
            files: 1,
          },
          attention: [{ kind: "human_review", closable: false }],
        }),
      ),
    ).toMatchObject({
      lead: "P-7 v2 waits for your review",
      rest: "· 1 file",
    });
    expect(
      proposalStrip(
        taskView("T-1", {
          status: "review",
          proposal: { ...proposal, title: "", files: 0 },
        }),
      )?.rest,
    ).toBe("");
  });

  it("waits for any reviewer while no reviewer holds the review claim", () => {
    expect(
      proposalStrip(taskView("T-1", { status: "review", proposal })),
    ).toEqual({
      proposal: "P-7",
      lead: "P-7 v2 waits for a reviewer",
      rest: "· Add the form · 3 files",
      waitsForYou: false,
    });
  });
});
