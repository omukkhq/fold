import { describe, expect, it } from "vitest";
import type { CheckResult, CheckRun } from "@/api/proposals";
import { proposalSummary } from "@/test/proposals";
import {
  checksFailedNote,
  logTail,
  outcomeText,
  queuedNote,
  reasonText,
  resultOf,
  runViews,
} from "./checkRuns";

function result(fields: Partial<CheckResult> = {}): CheckResult {
  return {
    name: "test",
    setup: false,
    required: true,
    outcome: "passed",
    exit_code: 0,
    duration_ms: 1200,
    log_bytes: 9,
    log: "all good\n",
    truncated: false,
    ...fields,
  };
}

function run(fields: Partial<CheckRun> = {}): CheckRun {
  return {
    id: "cr-4",
    stage: "proposal",
    purpose: "create_proposal",
    task: "T-3",
    version: null,
    base: "r7",
    state: "passed",
    reason: null,
    created_at: 1,
    finished_at: 2,
    results: [result()],
    ...fields,
  };
}

describe("outcomeText", () => {
  it("names each outcome with its exit code and time", () => {
    expect(outcomeText(result())).toBe("passed · 1.2 s");
    expect(outcomeText(result({ outcome: "failed", exit_code: 4 }))).toBe(
      "failed · exit 4 · 1.2 s",
    );
    expect(
      outcomeText(
        result({ outcome: "failed", exit_code: null, duration_ms: 40 }),
      ),
    ).toBe("failed · 40 ms");
    expect(outcomeText(result({ outcome: "timed_out" }))).toBe(
      "timed out · 1.2 s",
    );
    expect(outcomeText(result({ outcome: "skipped", required: false }))).toBe(
      "skipped: no changed file matches its pattern · not required",
    );
    expect(outcomeText(result({ outcome: "not_run" }))).toBe(
      "not run: the setup failed",
    );
  });
});

describe("runViews", () => {
  it("titles each run by its stage and names why a run failed", () => {
    const views = runViews([
      run({
        id: "cr-9",
        stage: "integration",
        purpose: "integrate",
        version: 2,
        base: "r9",
        state: "failed",
        reason: "check_failed",
        results: [result({ name: "e2e", outcome: "failed", exit_code: 1 })],
      }),
      run({ results: [result({ name: "lint", log_bytes: 0, log: "" })] }),
    ]);
    expect(views.map((view) => view.title)).toEqual([
      "Integration checks of v2 on r9",
      "Proposal checks on r7",
    ]);
    expect(views[0]?.note).toBe("a required check failed");
    expect(views[0]?.lines[0]).toEqual({
      name: "e2e",
      outcome: "failed",
      text: "failed · exit 1 · 1.2 s",
      hasLog: true,
    });
    expect(views[1]?.note).toBeNull();
    expect(views[1]?.lines[0]?.hasLog).toBe(false);
  });

  it("has a sentence for every reason", () => {
    for (const reason of [
      "check_failed",
      "setup_failed",
      "checks_file_invalid",
      "files_changed",
      "workspace_changed",
      "refused",
      "daemon_stopped",
      "superseded",
      "fold_failed",
    ] as const) {
      expect(reasonText(reason)).not.toBe("");
    }
    expect(reasonText("fold_failed")).toContain("integrate again");
  });
});

describe("logTail and resultOf", () => {
  it("keeps the last lines and marks a cut start", () => {
    const log = Array.from({ length: 5 }, (_, i) => `line ${String(i)}`).join(
      "\n",
    );
    expect(logTail(result({ log: `${log}\n` }), 2)).toBe("…\nline 3\nline 4");
    expect(logTail(result({ log: "one\n" }))).toBe("one");
    expect(logTail(result({ log: "end\n", truncated: true }))).toBe("…\nend");
  });

  it("finds a result by its name in a run read with its logs", () => {
    const read = run({ results: [result(), result({ name: "e2e" })] });
    expect(resultOf(read, "e2e")?.name).toBe("e2e");
    expect(resultOf(read, "none")).toBeUndefined();
    expect(resultOf(undefined, "e2e")).toBeUndefined();
  });
});

describe("queuedNote and checksFailedNote", () => {
  const open = proposalSummary("P-3");
  const queued = { integration_queued: true };

  it("says a queued integration waits for its checks while the proposal is open", () => {
    expect(queuedNote(proposalSummary("P-3", queued))).toMatch(/^Queued:/);
    expect(queuedNote(open)).toBeNull();
    expect(
      queuedNote(proposalSummary("P-3", { ...queued, state: "integrated" })),
    ).toBeNull();
    expect(
      queuedNote(proposalSummary("P-3", { ...queued, checks_failed: true })),
    ).toBeNull();
  });

  it("says failed integration checks send the proposal back to its author", () => {
    expect(checksFailedNote(open)).toBeNull();
    expect(
      checksFailedNote(
        proposalSummary("P-3", { checks_failed: true, current_version: 2 }),
      ),
    ).toBe(
      "The integration checks of v2 failed on the merge with main. The author sends a new version.",
    );
    expect(
      checksFailedNote(
        proposalSummary("P-3", { checks_failed: true, state: "closed" }),
      ),
    ).toBeNull();
  });
});
