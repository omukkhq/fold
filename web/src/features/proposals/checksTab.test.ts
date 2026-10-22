import { describe, expect, it } from "vitest";
import { checkResult, checkRun } from "@/test/proposals";
import {
  allRuns,
  chosenStep,
  firstError,
  isErrorLine,
  logLines,
  openingStep,
  runTitle,
} from "./checksTab";

const failedV1 = checkRun(82, {
  state: "failed",
  created_at: 100,
  results: [
    checkResult("setup"),
    checkResult("web", { outcome: "failed", exit_code: 2 }),
    checkResult("docker", { outcome: "not_run" }),
  ],
});
const passedV4 = checkRun(83, {
  created_at: 400,
  results: [checkResult("setup"), checkResult("web")],
});
const integrationV4 = checkRun(84, {
  stage: "integration",
  version: 4,
  base: "r52",
  created_at: 500,
  results: [checkResult("setup")],
});

describe("allRuns", () => {
  it("lists every run once, newest first, with the version it checked", () => {
    const items = allRuns(
      new Map([
        [1, [failedV1]],
        [4, [integrationV4, passedV4]],
        [3, [integrationV4]],
      ]),
    );
    expect(items.map((i) => [i.run.id, i.version, runTitle(i)])).toEqual([
      ["cr-84", 4, "v4 integration checks on r52"],
      ["cr-83", 4, "v4 proposal checks"],
      ["cr-82", 1, "v1 proposal checks"],
    ]);
  });

  it("orders runs of the same time by number", () => {
    const a = checkRun(5, { created_at: 1 });
    const b = checkRun(6, { created_at: 1 });
    expect(allRuns(new Map([[1, [a, b]]])).map((i) => i.run.id)).toEqual([
      "cr-6",
      "cr-5",
    ]);
  });
});

describe("chosenStep", () => {
  const items = allRuns(
    new Map([
      [1, [failedV1]],
      [4, [passedV4]],
    ]),
  );

  it("opens the failed step of the newest failed run", () => {
    expect(chosenStep(items, {})).toEqual({ run: "cr-82", step: "web" });
  });

  it("opens the step the address names, or the opening step of its run", () => {
    expect(chosenStep(items, { run: "cr-83", step: "web" })).toEqual({
      run: "cr-83",
      step: "web",
    });
    expect(chosenStep(items, { run: "cr-83", step: "lint" })).toEqual({
      run: "cr-83",
      step: "setup",
    });
    expect(chosenStep(items, { run: "cr-99" })).toEqual({
      run: "cr-82",
      step: "web",
    });
  });

  it("opens the newest run's first step when no run failed, and nothing without runs", () => {
    const passed = allRuns(new Map([[4, [passedV4]]]));
    expect(chosenStep(passed, {})).toEqual({ run: "cr-83", step: "setup" });
    expect(chosenStep([], {})).toBeNull();
    expect(openingStep(checkRun(1, { results: [] }))).toBeNull();
  });
});

describe("log lines", () => {
  it("numbers the lines and marks the errors", () => {
    const lines = logLines(
      "Checked 279 files.\nError: ENOTEMPTY\n46 passed, 0 failed\n ERR_PNPM_RECURSIVE_RUN_FIRST_FAIL\n\n",
    );
    expect(lines).toEqual([
      { n: 1, text: "Checked 279 files.", error: false },
      { n: 2, text: "Error: ENOTEMPTY", error: true },
      { n: 3, text: "46 passed, 0 failed", error: false },
      { n: 4, text: " ERR_PNPM_RECURSIVE_RUN_FIRST_FAIL", error: true },
    ]);
    expect(firstError(lines)).toBe(2);
    expect(firstError(lines.slice(0, 1))).toBeNull();
    expect(logLines("\n")).toEqual([]);
  });

  it("reads the usual failure words", () => {
    expect(isErrorLine("thread 'x' panicked at src/a.rs")).toBe(true);
    expect(isErrorLine("✕ renders the board")).toBe(true);
    expect(isErrorLine("test result: FAILED. 1 passed")).toBe(true);
    expect(isErrorLine("no errors found")).toBe(false);
    expect(isErrorLine("compiling errorless")).toBe(false);
  });
});
