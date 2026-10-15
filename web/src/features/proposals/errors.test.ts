import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { describeFailure, reloadsProposal } from "./errors";

const refusal = (code: string, details: Record<string, unknown> = {}) =>
  new ApiError(code, `refused: ${code}`, details);

describe("describeFailure", () => {
  it("shows a refusal of the author or of a stale version on the action that caused it", () => {
    expect(describeFailure(refusal("author_cannot_integrate"))).toEqual({
      place: "inline",
      text: "Your agent wrote this proposal, so you cannot integrate it.",
      mapped: true,
    });
    expect(describeFailure(refusal("author_cannot_review")).place).toBe(
      "inline",
    );
    expect(
      describeFailure(
        refusal("stale_version", { expected_version: 1, current_version: 2 }),
      ).text,
    ).toBe("v2 is now the current version. Read it, then try once more.");
    expect(describeFailure(refusal("stale_version")).text).toBe(
      "A newer version arrived. Read it, then try once more.",
    );
  });

  it("has an entry for every integrate gate code", () => {
    for (const code of [
      "not_approved",
      "human_approval_missing",
      "prerequisites_not_done",
      "changes_requested",
      "head_moved",
      "integration_conflicted",
    ]) {
      const failure = describeFailure(refusal(code));
      expect(failure.mapped, code).toBe(true);
      expect(failure.place, code).toBe("inline");
      expect(failure.text, code).not.toContain("refused:");
    }
  });

  it("reads the gate details it names", () => {
    expect(
      describeFailure(refusal("human_approval_missing", { reviewer: "ana" }))
        .text,
    ).toBe("fold waits for ana to approve this version.");
    expect(describeFailure(refusal("human_approval_missing")).text).toBe(
      "fold waits for a human to approve this version.",
    );
    expect(
      describeFailure(
        refusal("prerequisites_not_done", {
          prerequisites: [
            { task: "T-2", status: "review" },
            { task: "T-5", status: "todo" },
            "junk",
          ],
        }),
      ).text,
    ).toBe("T-2, T-5 are not Done, so fold did not integrate.");
    expect(
      describeFailure(
        refusal("prerequisites_not_done", {
          prerequisites: [{ task: "T-2", status: "review" }],
        }),
      ).text,
    ).toBe("T-2 is not Done, so fold did not integrate.");
    expect(
      describeFailure(refusal("prerequisites_not_done", { prerequisites: 3 }))
        .text,
    ).toBe("A prerequisite is not Done, so fold did not integrate.");
  });

  it("tells a version that arrived during the review from another invalid request", () => {
    expect(
      describeFailure(refusal("invalid_request", { reason: "version_changed" }))
        .text,
    ).toBe("A new version arrived during the review. Read it first.");
    expect(
      describeFailure(refusal("invalid_request", { field: "body" })).text,
    ).toBe("fold refused the request as written.");
  });

  it("shows the comment refusals on the form and the session refusals in a banner", () => {
    for (const code of [
      "empty_review",
      "unknown_comment_path",
      "invalid_comment_range",
      "invalid_path",
      "proposal_not_open",
      "task_closed",
      "integration_failed",
    ]) {
      expect(describeFailure(refusal(code)).place, code).toBe("inline");
    }
    for (const code of [
      "human_name_required",
      "auth_required",
      "proposal_not_found",
      "project_unavailable",
    ]) {
      expect(describeFailure(refusal(code))).toMatchObject({
        place: "banner",
        mapped: true,
      });
    }
  });

  it("shows a code without an entry in the generic banner with the server's message", () => {
    expect(
      describeFailure(new ApiError("disk_full", "The disk is full.")),
    ).toEqual({ place: "banner", text: "The disk is full.", mapped: false });
    expect(describeFailure(new ApiError("disk_full", ""))).toMatchObject({
      place: "banner",
      mapped: false,
    });
    expect(describeFailure(new TypeError("Failed to fetch")).text).toBe(
      "Failed to fetch",
    );
    expect(describeFailure("nothing").text).toBe(
      "The request did not reach the daemon. Try again.",
    );
  });

  it("reads a coded body that is not an ApiError without details", () => {
    expect(describeFailure({ error: { code: "stale_version" } }).text).toBe(
      "A newer version arrived. Read it, then try once more.",
    );
  });
});

describe("reloadsProposal", () => {
  it("reads the proposal again after a refusal that says it changed", () => {
    expect(reloadsProposal(refusal("stale_version"))).toBe(true);
    expect(reloadsProposal(refusal("integration_conflicted"))).toBe(true);
    expect(reloadsProposal(refusal("proposal_not_open"))).toBe(true);
    expect(reloadsProposal(refusal("invalid_request"))).toBe(true);
    expect(reloadsProposal(refusal("not_approved"))).toBe(false);
    expect(reloadsProposal(new Error("x"))).toBe(false);
  });
});
