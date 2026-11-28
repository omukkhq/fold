import { apiErrorCode } from "@/api/errors";
import type { CheckRun } from "@/api/proposals";
import { failureText } from "@/format/failure";

/**
 * Whether a run offers Stop to the signed-in human: a proposal-stage run in the `running` state, which includes a run that waits for memory, a check slot, a run slot or its setup. The daemon cancels no integration run, and the daemon still decides each cancel.
 */
export function showsStop(run: CheckRun, human: string | null): boolean {
  return human !== null && run.stage === "proposal" && run.state === "running";
}

export interface StopInput {
  /** The cancel request is on its way. */
  sending: boolean;
  /** The daemon accepted the cancel; the run has not ended yet in the data on screen. */
  accepted: boolean;
}

/** The button stays pending from the click until the data on screen shows the run ended, which the `checks.finished` event brings; the answer of the cancel route alone is no result. */
export function stopPending(input: StopInput): boolean {
  return input.sending || input.accepted;
}

export interface StopFailure {
  code: string | null;
  text: string;
}

/** A refused cancel, worded for its code. */
export function stopFailure(error: unknown): StopFailure {
  const code = apiErrorCode(error);
  switch (code) {
    case "check_run_ended":
      return { code, text: "The run ended before the cancel reached it." };
    case "check_run_not_found":
      return { code, text: "fold has no check run with this id." };
    case "invalid_request":
      return { code, text: "fold does not cancel this kind of run." };
    default:
      return { code, text: failureText(error) };
  }
}
