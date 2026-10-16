import { describe, expect, it } from "vitest";
import type { TaskReport } from "@/api/tasks";
import { taskView } from "@/test/payloads";
import {
  leadCheck,
  needsProposal,
  reportedText,
  resolvedText,
} from "./leadCheck";

const MIN = 60_000;

function report(over: Partial<TaskReport> = {}): TaskReport {
  return {
    by: "web-coder",
    note: "Drew the board screens.",
    at: 1_000,
    resolution: null,
    resolved_by: null,
    resolved_at: null,
    send_back_note: null,
    ...over,
  };
}

describe("leadCheck", () => {
  it("is null for a task that ends with a proposal", () => {
    expect(leadCheck(taskView("T-1"))).toBeNull();
    expect(needsProposal(taskView("T-1"))).toBe(true);
  });

  it("is null for a human task, which has no proposal either", () => {
    const human = taskView("T-2", { kind: "human" });
    expect(leadCheck(human)).toBeNull();
    expect(needsProposal(human)).toBe(false);
  });

  it("follows the newest report of a task without a proposal", () => {
    const task = (reports: TaskReport[], archived_at: number | null = null) =>
      taskView("T-1", { kind: "no_proposal", reports, archived_at });
    expect(needsProposal(task([]))).toBe(false);
    expect(leadCheck(task([]))).toEqual({
      state: "not_reported",
      title: "Not reported yet",
      report: null,
      canCheck: false,
    });
    const waiting = leadCheck(task([report()]));
    expect(waiting?.state).toBe("waiting");
    expect(waiting?.title).toBe("Waits for a lead check");
    expect(waiting?.canCheck).toBe(true);
    expect(leadCheck(task([report()], 5_000))?.canCheck).toBe(false);
    const sentBack = report({
      resolution: "sent_back",
      resolved_by: "human:ana",
      resolved_at: 2_000,
      send_back_note: "Use the tokens.",
    });
    expect(leadCheck(task([sentBack]))).toMatchObject({
      state: "sent_back",
      title: "Sent back to the coder",
      canCheck: false,
    });
    const completed = report({
      resolution: "completed",
      resolved_by: "web-lead",
      resolved_at: 3_000,
    });
    expect(leadCheck(task([completed, sentBack]))).toMatchObject({
      state: "completed",
      title: "Completed by a lead check",
      report: completed,
    });
  });
});

describe("report lines", () => {
  it("names the coder and the checker with the time", () => {
    const now = 1_000 + 2 * MIN;
    expect(reportedText(report(), now)).toEqual([
      "Reported by ",
      { actor: "web-coder" },
      " · 2 min ago",
    ]);
    expect(resolvedText(report(), now)).toBeNull();
    expect(
      resolvedText(
        report({
          resolution: "sent_back",
          resolved_by: "human:ana",
          resolved_at: 1_000 + MIN,
        }),
        now,
      ),
    ).toEqual(["Sent back by ", { actor: "human:ana" }, " · 1 min ago"]);
    expect(
      resolvedText(
        report({
          resolution: "completed",
          resolved_by: "web-lead",
          resolved_at: 1_000,
        }),
        now,
      )?.[0],
    ).toBe("Completed by ");
    expect(
      resolvedText(report({ resolution: "completed", resolved_by: null }), now),
    ).toBeNull();
  });
});
