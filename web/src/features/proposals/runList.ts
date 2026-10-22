import type { CheckResult, CheckRun } from "@/api/proposals";

/** A run as the Checks tab lists it, with the version it checked. */
export interface RunItem {
  run: CheckRun;
  /** The version whose proposal read listed the run; an integration run names its own. */
  version: number;
}

function runNumber(id: string): number {
  return Number(id.slice(id.indexOf("-") + 1));
}

/** Every run of every version once, newest first. */
export function allRuns(
  byVersion: ReadonlyMap<number, readonly CheckRun[]>,
): RunItem[] {
  const seen = new Map<string, RunItem>();
  for (const [version, runs] of byVersion) {
    for (const run of runs) {
      if (!seen.has(run.id)) {
        seen.set(run.id, { run, version: run.version ?? version });
      }
    }
  }
  return [...seen.values()].sort(
    (a, b) =>
      b.run.created_at - a.run.created_at ||
      runNumber(b.run.id) - runNumber(a.run.id),
  );
}

/** `v4 proposal checks`, `v4 integration checks on r52`, or `batch rb-2 integration checks on r52`. A batch's run checked every member together and is stored on the last of them, so on another member's page its version would name the wrong proposal. */
export function runTitle(item: RunItem): string {
  if (item.run.stage !== "integration") {
    return `v${String(item.version)} proposal checks`;
  }
  const who =
    item.run.batch === null
      ? `v${String(item.version)}`
      : `batch ${item.run.batch}`;
  return `${who} integration checks on ${item.run.base}`;
}

export interface StepRef {
  run: string;
  step: string;
}

function broke(result: CheckResult): boolean {
  return result.outcome === "failed" || result.outcome === "timed_out";
}

/** The step a run opens at: the one that broke, else its first. */
export function openingStep(run: CheckRun): StepRef | null {
  const result = run.results.find(broke) ?? run.results[0];
  return result ? { run: run.id, step: result.name } : null;
}

/**
 * The step the tab shows: the one the address names when its run has it, the step that broke in the run the address names, else the failed step of the newest failed run, else the first step of the newest run.
 */
export function chosenStep(
  items: readonly RunItem[],
  search: { run?: string; step?: string },
): StepRef | null {
  const named = items.find((item) => item.run.id === search.run)?.run;
  if (named) {
    const step = named.results.find((r) => r.name === search.step);
    return step ? { run: named.id, step: step.name } : openingStep(named);
  }
  const failed = items.find((item) => item.run.results.some(broke))?.run;
  if (failed) return openingStep(failed);
  const newest = items[0]?.run;
  return newest ? openingStep(newest) : null;
}

/** One numbered line of a stored log, marked when it reads as an error. */
export interface LogLine {
  n: number;
  text: string;
  error: boolean;
}

const ERROR_WORDS =
  /\b(error|errors|failed|failure|fail|panicked|fatal|exception|traceback)\b|^\s*(✕|✗|×)|\bERR_/i;
const NO_ERRORS = /\b(0|no) (errors?|failed|failures?)\b/i;

/** Whether a log line reads as an error; a count of none, such as `0 failed`, does not. */
export function isErrorLine(text: string): boolean {
  return ERROR_WORDS.test(text) && !NO_ERRORS.test(text);
}

/** The stored log as numbered lines; a trailing line break adds no line. */
export function logLines(log: string): LogLine[] {
  const body = log.replace(/\n+$/, "");
  if (body === "") return [];
  return body
    .split("\n")
    .map((text, index) => ({ n: index + 1, text, error: isErrorLine(text) }));
}

/** The number of the first error line, or null. */
export function firstError(lines: readonly LogLine[]): number | null {
  return lines.find((line) => line.error)?.n ?? null;
}

function seconds(ms: number): string {
  return ms < 1000 ? `${String(ms)} ms` : `${(ms / 1000).toFixed(1)} s`;
}

/** A step's short note in the run list: `1.2 s`, `exit 2 · 1.3 s`, `not run`. */
export function stepMeta(result: CheckResult): string {
  switch (result.outcome) {
    case "passed":
      return seconds(result.duration_ms);
    case "failed":
      return result.exit_code === null
        ? seconds(result.duration_ms)
        : `exit ${String(result.exit_code)} · ${seconds(result.duration_ms)}`;
    case "timed_out":
      return `timed out · ${seconds(result.duration_ms)}`;
    case "skipped":
      return "skipped";
    case "not_run":
      return "not run";
  }
}
