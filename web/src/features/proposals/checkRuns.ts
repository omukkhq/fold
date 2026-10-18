import type {
  CheckOutcome,
  CheckResult,
  CheckRun,
  CheckRunReason,
  CheckRunState,
  ProposalSummary,
} from "@/api/proposals";

/** The most log lines the checks section shows; the stored log keeps more. */
export const LOG_LINES = 60;

/** One command of a run as the checks section lists it. */
export interface CheckLine {
  name: string;
  outcome: CheckOutcome;
  /** What happened, such as "failed · exit 4 · 1.2 s". */
  text: string;
  /** A command whose stored log end the section can show. */
  hasLog: boolean;
}

/** One run as the checks section lists it, newest first. */
export interface RunView {
  id: string;
  title: string;
  state: CheckRunState;
  /** Why the run ended as it did; null for a passed or running run. */
  note: string | null;
  lines: CheckLine[];
}

const REASONS: Record<CheckRunReason, string> = {
  check_failed: "a required check failed",
  setup_failed: "the setup failed, so no check ran",
  checks_file_invalid: "fold-checks.toml is not valid",
  files_changed: "a check changed recorded files",
  workspace_changed: "the workspace changed during the run",
  refused: "the checks passed, but the command was refused by then",
  daemon_stopped: "the daemon stopped during the run",
  superseded: "the integration started again on a newer state",
  fold_failed:
    "fold could not run the checks; the version keeps its place, so integrate again once the fault is fixed",
};

/** The sentence for why a run failed or stopped. */
export function reasonText(reason: CheckRunReason): string {
  return REASONS[reason];
}

function seconds(ms: number): string {
  return ms < 1000 ? `${String(ms)} ms` : `${(ms / 1000).toFixed(1)} s`;
}

/** What one command did, in a short line. */
export function outcomeText(result: CheckResult): string {
  const optional = result.required ? "" : " · not required";
  switch (result.outcome) {
    case "passed":
      return `passed · ${seconds(result.duration_ms)}${optional}`;
    case "failed":
      return result.exit_code === null
        ? `failed · ${seconds(result.duration_ms)}${optional}`
        : `failed · exit ${String(result.exit_code)} · ${seconds(result.duration_ms)}${optional}`;
    case "timed_out":
      return `timed out · ${seconds(result.duration_ms)}${optional}`;
    case "skipped":
      return `skipped: no changed file matches its pattern${optional}`;
    case "not_run":
      return "not run: the setup failed";
  }
}

function title(run: CheckRun): string {
  const version = run.version === null ? "" : ` of v${String(run.version)}`;
  return run.stage === "integration"
    ? `Integration checks${version} on ${run.base}`
    : `Proposal checks on ${run.base}`;
}

/** The runs of the shown version as the section lists them, in the order the daemon sends them: newest first. */
export function runViews(runs: readonly CheckRun[]): RunView[] {
  return runs.map((run) => ({
    id: run.id,
    title: title(run),
    state: run.state,
    note: run.reason === null ? null : reasonText(run.reason),
    lines: run.results.map((result) => ({
      name: result.name,
      outcome: result.outcome,
      text: outcomeText(result),
      hasLog: result.log_bytes > 0,
    })),
  }));
}

/** The last `lines` lines of a stored log, with a first line that says when the start was cut. */
export function logTail(result: CheckResult, lines = LOG_LINES): string {
  const all = result.log.replace(/\n+$/, "").split("\n");
  const kept = all.slice(-lines);
  const cut = result.truncated || kept.length < all.length;
  return cut ? ["…", ...kept].join("\n") : kept.join("\n");
}

/** The result of `name` in a run read with its logs. */
export function resultOf(
  run: CheckRun | undefined,
  name: string,
): CheckResult | undefined {
  return run?.results.find((result) => result.name === name);
}

/** Why Integrate waits while the human's own integration is queued. */
export const QUEUED_BLOCK = "This integration is already queued.";

/** What the screen says while an integration of the current version waits in the daemon's queue for its checks, whoever asked for it. Null otherwise; the event stream reads the proposal again when the request ends. */
export function queuedNote(summary: ProposalSummary): string | null {
  if (
    !summary.integration_queued ||
    summary.state !== "open" ||
    summary.checks_failed
  ) {
    return null;
  }
  return "Queued: fold runs the integration checks on the merge with main, then integrates when they pass.";
}

/** What the screen says when the integration checks of the current version failed, so the proposal waits for its author. */
export function checksFailedNote(summary: ProposalSummary): string | null {
  if (summary.state !== "open" || !summary.checks_failed) return null;
  return `The integration checks of v${String(summary.current_version)} failed on the merge with main. The author sends a new version.`;
}
