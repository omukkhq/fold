import { describe, expect, it } from "vitest";
import {
  checkpointText,
  claimNote,
  clockTime,
  collapse,
  feedbackPlaceholder,
  historyText as historyTextSegments,
  humanReviewLines,
  integrationText as integrationTextSegments,
  messageCountText,
  parseRecipient,
  proposalStateText,
  recipientValue,
  reviewText,
  statusLook,
} from "./text";
import { segmentsText } from "@/people/segments";

const historyText = (...args: Parameters<typeof historyTextSegments>) =>
  segmentsText(historyTextSegments(...args));

const integrationText = (...args: Parameters<typeof integrationTextSegments>) =>
  segmentsText(integrationTextSegments(...args));

const AT = new Date(2026, 9, 15, 13, 40, 5).getTime();
const MINUTE = 60_000;

describe("statusLook", () => {
  it("names every status and takes its dot from the status token", () => {
    expect(statusLook("in_progress")).toMatchObject({
      text: "In Progress",
      dot: "bg-status-progress",
    });
    expect(
      (["backlog", "todo", "review", "done"] as const).map(
        (s) => statusLook(s).text,
      ),
    ).toEqual(["Backlog", "Todo", "Review", "Done"]);
  });
});

describe("times and claims", () => {
  it("prints the local time of day", () => {
    expect(clockTime(AT)).toBe("13:40");
    expect(clockTime(new Date(2026, 0, 1, 9, 5).getTime())).toBe("09:05");
  });

  it("says when the claim was taken and when its lease was renewed", () => {
    const claim = {
      kind: "implementation" as const,
      holder: "frontend/ui",
      claimed_at: AT,
      renewed_at: AT + 10 * MINUTE,
      stale: false,
    };
    expect(claimNote(claim, AT + 12 * MINUTE)).toBe(
      "Claimed 13:40 · lease renewed 2 min ago",
    );
    expect(claimNote({ ...claim, stale: true }, AT + 57 * MINUTE)).toBe(
      "Claimed 13:40 · lease stale, last renewed 47 min ago",
    );
  });
});

describe("historyText", () => {
  it("says who did what and when", () => {
    expect(
      historyText({
        type: "claim.acquired",
        actor: "frontend/ui",
        created_at: AT,
        seq: 1,
      }),
    ).toBe("Claimed by @frontend/ui · 13:40");
    expect(
      historyText({
        type: "task.created",
        actor: "human:tanim",
        created_at: AT,
        seq: 1,
      }),
    ).toBe("Created by @tanim · 13:40");
    expect(
      historyText({
        type: "task.hold_set",
        actor: "web-lead",
        created_at: AT,
        seq: 2,
      }),
    ).toBe("Blocker set by @web-lead · 13:40");
    expect(
      historyText({
        type: "task.hold_cleared",
        actor: "human:tanim",
        created_at: AT,
        seq: 3,
      }),
    ).toBe("Blocker cleared by @tanim · 13:40");
  });

  it("names a type that has no sentence", () => {
    expect(
      historyText({
        type: "remote.pushed",
        actor: "fold",
        created_at: AT,
        seq: 1,
      }),
    ).toBe("remote.pushed by @fold · 13:40");
  });
});

describe("collapse", () => {
  it("keeps the first entries and offers the rest", () => {
    expect(collapse([1, 2, 3, 4], 2, false)).toEqual({
      shown: [1, 2],
      more: "Show 2 more",
    });
    expect(collapse([1, 2, 3, 4], 2, true)).toEqual({
      shown: [1, 2, 3, 4],
      more: "Show fewer",
    });
  });

  it("offers nothing when nothing is hidden", () => {
    expect(collapse([1, 2], 2, false)).toEqual({ shown: [1, 2], more: null });
    expect(collapse([], 1, true)).toEqual({ shown: [], more: null });
  });
});

describe("checkpointText", () => {
  const ck = { id: "ck-1", created_at: AT };
  it("marks automatic checkpoints and shows explicit labels", () => {
    expect(
      checkpointText({ ...ck, automatic: true, label: "quiet 2 min" }),
    ).toBe("auto · quiet 2 min");
    expect(checkpointText({ ...ck, automatic: true, label: null })).toBe(
      "auto",
    );
    expect(checkpointText({ ...ck, automatic: false, label: "before" })).toBe(
      "before",
    );
    expect(checkpointText({ ...ck, automatic: false, label: null })).toBe(
      "checkpoint",
    );
  });
});

describe("humanReviewLines", () => {
  it("names the reviewer, or any human, and who set the mark and when the daemon says", () => {
    expect(
      humanReviewLines(
        { reviewer: "tanim", reason: "auth", set_by: "planner" },
        AT,
      ),
    ).toEqual({ reviewer: "human:tanim", setBy: "planner", at: "13:40" });
    expect(
      humanReviewLines(
        { reviewer: null, reason: "auth", set_by: "planner" },
        null,
      ),
    ).toEqual({ reviewer: null, setBy: "planner", at: null });
  });
});

describe("proposal texts", () => {
  it("say the verdict, the version and the integration", () => {
    expect(
      reviewText({
        reviewer: "review",
        verdict: "request_changes",
        version: 2,
      }),
    ).toEqual([{ actor: "review" }, " requested changes on v2"]);
    expect(
      (["comment", "approve", "reject"] as const).map((verdict) =>
        segmentsText(reviewText({ reviewer: "r", verdict, version: 1 })),
      ),
    ).toEqual(["@r commented on v1", "@r approved on v1", "@r rejected on v1"]);
    expect(proposalStateText("open", 3)).toBe("waiting for review · v3");
    expect(proposalStateText("conflict", 1)).toBe("conflict · v1");
    expect(proposalStateText("open", 2, "review-lead")).toBe(
      "waiting for review · v2 · with @review-lead",
    );
    expect(
      integrationText({
        revision: "r8",
        proposal: "P-5",
        by: "review",
        at: AT,
        commit: null,
      }),
    ).toBe("by @review · 13:40");
    expect(messageCountText(2, "T-12")).toBe("2 linked to T-12");
  });
});

describe("feedback recipients", () => {
  it("round-trip through the select value, keeping the holder and an agent named holder apart", () => {
    for (const to of [
      { kind: "holder" as const },
      { kind: "agent" as const, name: "frontend" },
      { kind: "agent" as const, name: "holder" },
    ]) {
      expect(parseRecipient(recipientValue(to))).toEqual(to);
    }
    expect(recipientValue({ kind: "agent", name: "holder" })).toBe(
      "agent:holder",
    );
  });

  it("refuse a malformed value", () => {
    for (const value of [
      "",
      "agent:",
      ":x",
      "human:tanim",
      "tag:ui",
      "frontend",
    ]) {
      expect(parseRecipient(value), value).toBeNull();
    }
  });

  it("name the reader in the placeholder", () => {
    expect(feedbackPlaceholder({ kind: "holder" }, "frontend/ui")).toBe(
      "Write feedback to @frontend/ui…",
    );
    expect(feedbackPlaceholder({ kind: "holder" }, null)).toBe(
      "Write feedback to the claim holder…",
    );
    expect(feedbackPlaceholder({ kind: "agent", name: "review" }, null)).toBe(
      "Write feedback to review…",
    );
    expect(feedbackPlaceholder(null, null)).toBe(
      "Choose who reads the feedback",
    );
  });
});
