import type { Session } from "../../api/session";

/** `token` when the request carries no valid token, `name` when it is authenticated and the human has no name yet, `ready` otherwise. */
export type SignInMode = "token" | "name" | "ready";

export function signInMode(session: Session): SignInMode {
  if (!session.authenticated) {
    return "token";
  }
  return session.human === null ? "name" : "ready";
}
