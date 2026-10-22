import type { Session } from "../../api/session";

/** `ready` when the request is signed in as a human, `token` otherwise: the sign-in screen then opens a link or asks for a token. */
export type SignInMode = "token" | "ready";

export function signInMode(session: Session): SignInMode {
  return session.authenticated && session.human !== null ? "ready" : "token";
}
