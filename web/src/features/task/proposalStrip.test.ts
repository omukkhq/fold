import { describe, expect, it } from "vitest";
import { taskView } from "@/test/payloads";
import { proposalStrip } from "./proposalStrip";

const proposal = {
  id: "P-7",
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
      waitsForYou: true,
    });
  });

  it("is neutral while a reviewer agent has the version", () => {
    expect(proposalStrip(taskView("T-1", { proposal }))).toEqual({
      proposal: "P-7",
      lead: "P-7 v2 waits for a reviewer",
      waitsForYou: false,
    });
  });
});
