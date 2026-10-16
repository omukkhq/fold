import { apiErrorCode } from "@/api/errors";

/** The sentences for the codes that every screen words the same way. */
const COMMON_TEXTS: Readonly<Record<string, string>> = {
  recipient_not_found: "No agent can receive this message yet.",
  auth_required: "Sign in again, then try once more.",
  human_name_required: "Tell fold your name first, then try once more.",
  project_unavailable: "The project is not available now.",
};

/** The shared sentence for a code, or undefined when the screen words it itself. */
export function commonText(code: string): string | undefined {
  return COMMON_TEXTS[code];
}

const TEXTS: Record<string, string> = {
  ...COMMON_TEXTS,
  not_found: "The daemon does not serve this request yet.",
  agent_not_found: "fold has no agent with this name.",
  message_not_found: "The message this replies to does not exist.",
  task_not_found: "No task has this id.",
  invalid_token: "Sign in again, then try once more.",
  human_required: "Only a signed-in human can do this.",
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
