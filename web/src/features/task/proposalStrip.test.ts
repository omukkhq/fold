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
  it("is absent without a proposal and after the integration", () => {
    expect(proposalStrip(taskView("T-1"))).toBeNull();
    expect(
      proposalStrip(
        taskView("T-1", { proposal: { ...proposal, state: "integrated" } }),
      ),
    ).toBeNull();
  });

  it("is gold and names the human while the version waits for human review", () => {
    expect(
      proposalStrip(
        taskView("T-1", { proposal, attention: [{ kind: "human_review" }] }),
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
          proposal: {
            ...proposal,
            review_claim: "review-lead",
            title: "",
            files: 1,
          },
          attention: [{ kind: "human_review" }],
        }),
      ),
    ).toMatchObject({
      lead: "P-7 v2 waits for your review",
      rest: "· 1 file",
    });
    expect(
      proposalStrip(
        taskView("T-1", { proposal: { ...proposal, title: "", files: 0 } }),
      )?.rest,
    ).toBe("");
  });

  it("is neutral while a reviewer agent has the version", () => {
    expect(proposalStrip(taskView("T-1", { proposal }))).toEqual({
      proposal: "P-7",
      lead: "P-7 v2 waits for a reviewer",
      rest: "· Add the form · 3 files",
      waitsForYou: false,
    });
  });
});
