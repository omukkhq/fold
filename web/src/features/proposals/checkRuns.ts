import type {
  CheckResult,
  CheckRun,
  CheckRunReason,
  ProposalSummary,
} from "@/api/proposals";

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
