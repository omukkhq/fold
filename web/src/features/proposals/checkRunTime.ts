import type { CheckRun } from "@/api/proposals";
import { formatAsOfMinute, formatDateTime } from "@/format/time";
import { formatStepDuration } from "./runList";

const MINUTE = 60_000;

/** The time line of a run header and the full times for its tooltip. */
export interface RunTime {
  label: string;
  tooltip: string;
}

/** A run's length: the step format (`11 ms`, `83.2 s`) under a minute, else whole minutes (`9 min`). */
export function runDuration(ms: number): string {
  const length = Math.max(0, ms);
  return length < MINUTE
    ? formatStepDuration(length)
    : `${String(Math.floor(length / MINUTE))} min`;
}

/**
 * What a run header says about time. `created_at` is when the run started. A run that has an end time shows how long it took; a run with none shows its start, and `running` only while its state says so.
 */
export function runTime(run: CheckRun, now: number): RunTime {
  const started = `Started ${formatAsOfMinute(now, run.created_at)}`;
  const startedFull = `Started ${formatDateTime(run.created_at)}`;
  if (run.finished_at !== null) {
    return {
      label: `${started} · took ${runDuration(run.finished_at - run.created_at)}`,
      tooltip: `${startedFull} · Finished ${formatDateTime(run.finished_at)}`,
    };
  }
  return {
    label: run.state === "running" ? `${started} · running` : started,
    tooltip: startedFull,
  };
}
