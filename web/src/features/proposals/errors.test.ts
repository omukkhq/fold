import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { commonText } from "@/format/failure";
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

  it("shows a human approval that comes before a reviewer agent's on the action", () => {
    expect(describeFailure(refusal("agent_approval_required"))).toEqual({
      place: "inline",
      text: "A reviewer agent approves this version first, then you can approve it.",
      mapped: true,
    });
  });

  it("names the run of the next version that holds Approve and Integrate", () => {
    const text = (details: Record<string, unknown>) =>
      describeFailure(refusal("update_checks_running", details));
    expect(
      text({ version: 2, run: "cr-4", purpose: "update_proposal" }),
    ).toEqual({
      place: "inline",
      text: "v3 is running its checks. Try again when they end.",
      mapped: true,
    });
    expect(text({ version: 2, purpose: "review_fix" }).text).toBe(
      "v3, a reviewer's fix, is running its checks. Try again when they end.",
    );
    expect(text({ purpose: "review_fix" }).text).toBe(
      "A reviewer's fix is running its checks. Try again when they end.",
    );
    expect(text({}).text).toBe(
      "The next version is running its checks. Try again when they end.",
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

  it("words each refusal of a human edit by its reason", () => {
    const text = (code: string, details: Record<string, unknown> = {}) =>
      describeFailure(refusal(code, details));
    expect(
      text("human_edit_denied", { reviewer: "ana", proposal: "P-1" }),
    ).toEqual({
      place: "inline",
      text: "Only @ana can change this proposal in the human review.",
      mapped: true,
    });
    expect(text("human_edit_denied", { reviewer: null }).text).toBe(
      "You can change a proposal only in a human review.",
    );
    expect(text("worker_busy", { reason: "working" }).text).toBe(
      "The task worker is working in its workspace now. Try again when it stops.",
    );
    expect(text("worker_busy", { reason: "unfinished_write" }).text).toBe(
      "The task workspace waits for its worker to sync an earlier change. Try again after that.",
    );
    expect(text("worker_busy", { reason: "newer_work" }).text).toBe(
      "The task worker has work that this version does not hold. Ask for changes instead.",
    );
    expect(text("file_not_editable", { reason: "missing" }).text).toBe(
      "This version does not have that file.",
    );
    expect(text("file_not_editable", { reason: "too_large" }).text).toBe(
      "The file is too large to edit here.",
    );
    expect(text("file_not_editable", { reason: "binary" }).text).toBe(
      "Only a text file can be edited here.",
    );
    expect(text("version_unchanged").place).toBe("inline");
    expect(text("invalid_intent", { reason: "too_long" }).text).toBe(
      "The intent is too long. Keep it to 2000 characters.",
    );
    expect(text("invalid_intent", { reason: "empty" })).toMatchObject({
      place: "inline",
      text: "Say what the new version does, why, what it does not change and its risks.",
    });
  });

  it("reads the gate details it names", () => {
    expect(
      describeFailure(refusal("human_approval_missing", { reviewer: "ana" }))
        .text,
    ).toBe("fold waits for @ana to approve this version.");
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

  it("shows an invalid request in one sentence, and a version that arrived meanwhile as stale_version", () => {
    expect(
      describeFailure(refusal("invalid_request", { field: "body" })).text,
    ).toBe("fold refused the request as written.");
    expect(
      describeFailure(
        refusal("stale_version", { expected_version: 2, current_version: 3 }),
      ).text,
    ).toBe("v3 is now the current version. Read it, then try once more.");
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
    expect(describeFailure(refusal("project_unavailable")).text).toBe(
      commonText("project_unavailable"),
    );
    expect(describeFailure(refusal("recipient_not_found"))).toEqual({
      place: "banner",
      text: commonText("recipient_not_found"),
      mapped: true,
    });
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
    expect(reloadsProposal(refusal("update_checks_running"))).toBe(true);
    expect(reloadsProposal(refusal("not_approved"))).toBe(false);
    expect(reloadsProposal(new Error("x"))).toBe(false);
  });
});
