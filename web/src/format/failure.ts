import { apiErrorCode } from "@/api/errors";

const TEXTS: Record<string, string> = {
  not_found: "The daemon does not serve this request yet.",
  agent_not_found: "fold has no agent with this name.",
  message_not_found: "The message this replies to does not exist.",
  recipient_not_found: "The message has no recipient that fold knows.",
  task_not_found: "No task has this id.",
  auth_required: "Sign in again, then try once more.",
  invalid_token: "Sign in again, then try once more.",
  human_required: "Only a signed-in human can do this.",
  human_name_required: "Tell fold your name first, then try once more.",
  project_unavailable: "The project is not available now.",
  invalid_request: "The daemon refused the request as written.",
  internal_error: "The daemon failed. Try again.",
};

/** A sentence for a failed human write. It branches on the envelope's code; an error without a code never reached the daemon. */
export function failureText(error: unknown): string {
  const code = apiErrorCode(error);
  if (code === null) {
    return "The request did not reach the daemon. Try again.";
  }
  return TEXTS[code] ?? "The daemon refused the request.";
}
