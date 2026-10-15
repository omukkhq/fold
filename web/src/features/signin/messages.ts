import { apiErrorCode } from "../../api/errors";

/** The inline message for a failed sign-in, chosen from the error code; the daemon's message text is never shown. */
export function signInErrorMessage(error: unknown): string {
  switch (apiErrorCode(error)) {
    case "invalid_token":
    case "auth_required":
      return "That token was not accepted. Check it and try again.";
    case "invalid_name":
      return "The daemon did not accept that name.";
    case null:
      return "The daemon did not answer. Try again.";
    default:
      return "Sign-in failed. Try again.";
  }
}
