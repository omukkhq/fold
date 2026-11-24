import { describe, expect, it } from "vitest";
import { checkRun } from "@/test/proposals";
import { runDuration, runTime } from "./checkRunTime";

const at = (d: number, h: number, m: number, s = 0) =>
  new Date(2026, 10, d, h, m, s).getTime();
const NOW = at(24, 15, 0);

describe("runDuration", () => {
  it("uses the step format under a minute and whole minutes after", () => {
    expect(runDuration(11)).toBe("11 ms");
    expect(runDuration(12_340)).toBe("12.3 s");
    expect(runDuration(59_999)).toBe("60.0 s");
    expect(runDuration(60_000)).toBe("1 min");
    expect(runDuration(9 * 60_000 + 59_000)).toBe("9 min");
    expect(runDuration(-5)).toBe("0 ms");
  });
});

describe("runTime", () => {
  it("says a running run started and still runs, without a duration", () => {
    const run = checkRun(1, {
      state: "running",
      created_at: at(24, 14, 0),
      finished_at: null,
    });
    expect(runTime(run, NOW)).toEqual({
      label: "Started 14:00 · running",
      tooltip: "Started Nov 24, 2026, 14:00:00",
    });
  });

  it("says how long an ended run took and gives both times", () => {
    const run = checkRun(2, {
      created_at: at(24, 14, 0, 5),
      finished_at: at(24, 14, 9, 35),
    });
    expect(runTime(run, NOW)).toEqual({
      label: "Started 14:00 · took 9 min",
      tooltip:
        "Started Nov 24, 2026, 14:00:05 · Finished Nov 24, 2026, 14:09:35",
    });
  });

  it("shows seconds for a run under a minute", () => {
    const run = checkRun(3, {
      created_at: at(24, 14, 0, 0),
      finished_at: at(24, 14, 0, 12) + 300,
    });
    expect(runTime(run, NOW).label).toBe("Started 14:00 · took 12.3 s");
  });

  it("names the day of a run that started on another day and ended on the next", () => {
    const run = checkRun(4, {
      created_at: at(23, 23, 55),
      finished_at: at(24, 0, 5),
    });
    expect(runTime(run, NOW)).toEqual({
      label: "Started Nov 23, 23:55 · took 10 min",
      tooltip:
        "Started Nov 23, 2026, 23:55:00 · Finished Nov 24, 2026, 00:05:00",
    });
  });

  it("shows only the start for a stopped run with no end time", () => {
    const run = checkRun(5, {
      state: "stopped",
      created_at: at(24, 14, 0),
      finished_at: null,
    });
    expect(runTime(run, NOW)).toEqual({
      label: "Started 14:00",
      tooltip: "Started Nov 24, 2026, 14:00:00",
    });
  });
});
