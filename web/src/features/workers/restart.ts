import { apiErrorCode } from "@/api/errors";
import type { RestartWorkerBody } from "@/api/workers";
import { failureText } from "@/format/failure";
import type { Failure } from "./startLead";

/** The request body: empty text is left out. */
export function restartBody(instructions: string): RestartWorkerBody {
  const text = instructions.trim();
  return text === "" ? {} : { instructions: text };
}

/** A refused restart, worded for its code. */
export function restartFailure(error: unknown): Failure {
  const code = apiErrorCode(error);
  switch (code) {
    case "project_paused":
      return {
        code,
        text: "The agents of this project are paused. Resume them first, then start the worker again.",
      };
    case "worker_limit_reached":
      return {
        code,
        text: "fold runs as many workers as the project allows. Stop one first, or ask the main agent to raise the limit.",
      };
    case "worker_exists":
      return {
        code,
        text: "The worker is starting in a new session already, or started again while fold ended its process. Look at its state, then try again.",
      };
    case "worker_busy":
      return {
        code,
        text: "A stop came first, so the worker stays stopped in its session. Start a new session again if it still needs one.",
      };
    case "worker_process_not_found":
      return { code, text: "fold runs no process for this worker." };
    case "invalid_config":
      return {
        code,
        text: "fold cannot read the worker settings in .fold/config.toml. Run `fld doctor`.",
      };
    default:
      return { code, text: failureText(error) };
  }
}
