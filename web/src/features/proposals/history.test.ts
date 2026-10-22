import { describe, expect, it } from "vitest";
import {
  checkResult,
  checkRun,
  proposalDetail,
  proposalSummary,
  proposalVersion,
  review,
} from "@/test/proposals";
import { excerpt, historyEntries, runSummary } from "./history";

const p = proposalDetail(proposalSummary("P-39", { current_version: 3 }), {
  versions: [
    proposalVersion(1, { base: "r40" }),
    proposalVersion(2, {
      base: "r40",
      reviewer_fix: true,
      created_by: "review/fix",
    }),
    proposalVersion(3, { base: "r42", human_edit: true }),
  ],
  reviews: [
    review(1, "review/quality", "request_changes", {
      body: "Two findings.\n\nStream after the state check.\nDo not log the key.\nAnd more.",
    }),
    review(1, "review/quality", "comment"),
    review(2, "review/quality", "approve", { body: "Fixed." }),
    review(3, "review/quality", "approve", { kept_from: 2 }),
    review(3, "human:tanim", "approve", { created_at: 2_000_000_000_000 }),
  ],
});

describe("historyEntries", () => {
  it("lists each version newest first with how it was made, its change and its reviews", () => {
    const entries = historyEntries(p, new Map([[3, [checkRun(9)]]]));
    expect(
      entries.map((e) => [
        e.version.version,
        e.current,
        e.verdict,
        e.origin,
        e.mergedMain,
        e.diff.label,
      ]),
    ).toEqual([
      [3, true, "approved", "human_edit", "r42", "Changes since v2"],
      [2, false, "approved", "review_fix", null, "Changes since v1"],
      [1, false, "changes requested", "author", null, "Full change"],
    ]);
    expect(entries.map((e) => e.diff.search)).toEqual([
      { tab: "changes", since: 2 },
      { tab: "changes", version: 2, since: 1 },
      { tab: "changes", version: 1 },
    ]);
    expect(entries.map((e) => e.runs?.map((r) => r.id) ?? null)).toEqual([
      ["cr-9"],
      null,
      null,
    ]);
  });

  it("groups the reviews by version, newest first, without the empty comment reviews", () => {
    const entries = historyEntries(p, new Map());
    expect(
      entries.map((e) =>
        e.reviews.map((r) => `${r.review.reviewer}: ${r.head}`),
      ),
    ).toEqual([
      ["human:tanim: Approved", "review/quality: Approved (kept from v2)"],
      ["review/quality: Approved"],
      ["review/quality: Changes requested"],
    ]);
    const first = entries[2]?.reviews[0];
    expect(first?.excerpt).toEqual({
      text: "Two findings.\nStream after the state check.\nDo not log the key.",
      cut: true,
    });
    expect(first?.scope).toBeNull();
  });
});

describe("excerpt", () => {
  it("keeps short texts whole and cuts long ones at a word", () => {
    expect(excerpt("  One line.  ")).toEqual({ text: "One line.", cut: false });
    expect(excerpt("")).toEqual({ text: "", cut: false });
    const long = `${"word ".repeat(80)}end`;
    const cut = excerpt(long, 3, 40);
    expect(cut.cut).toBe(true);
    expect(cut.text).toBe(`${"word ".repeat(8).trimEnd()}…`);
    expect(excerpt("x".repeat(50), 3, 40)).toEqual({
      text: `${"x".repeat(40)}…`,
      cut: true,
    });
  });
});

describe("runSummary", () => {
  const steps = (...names: string[]) => names.map((n) => checkResult(n));

  it("names the passed commands", () => {
    expect(runSummary(checkRun(1, { results: steps("setup", "web") }))).toBe(
      "2 of 2 passed · setup, web",
    );
    expect(
      runSummary(
        checkRun(1, {
          results: [
            ...steps("a", "b", "c", "d", "e", "f"),
            checkResult("g", { outcome: "skipped" }),
          ],
        }),
      ),
    ).toBe("6 of 7 passed · 1 skipped · a, b, c, d, e, f and 1 more");
    expect(runSummary(checkRun(1, { results: [] }))).toBe("passed");
  });

  it("names the failed command and what else happened", () => {
    expect(
      runSummary(
        checkRun(1, {
          state: "failed",
          results: [
            ...steps("setup", "backend"),
            checkResult("web", { outcome: "failed", exit_code: 2 }),
            checkResult("docker", { outcome: "not_run" }),
            checkResult("api", { outcome: "not_run" }),
          ],
        }),
      ),
    ).toBe("web failed (exit 2) · 2 passed, 2 not run");
    expect(
      runSummary(
        checkRun(1, {
          state: "failed",
          results: [checkResult("web", { outcome: "timed_out" })],
        }),
      ),
    ).toBe("web timed out");
    expect(
      runSummary(
        checkRun(1, {
          state: "failed",
          results: [checkResult("web", { outcome: "failed", exit_code: null })],
        }),
      ),
    ).toBe("web failed");
  });

  it("says a run is running or stopped", () => {
    expect(
      runSummary(
        checkRun(1, {
          state: "running",
          results: [checkResult("a"), checkResult("b", { outcome: "not_run" })],
        }),
      ),
    ).toBe("running · 1 of 2 passed so far");
    expect(
      runSummary(
        checkRun(1, {
          state: "stopped",
          results: [checkResult("a"), checkResult("b", { outcome: "not_run" })],
        }),
      ),
    ).toBe("stopped · 1 passed · 1 not run");
  });
});
