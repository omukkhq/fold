import { API_BASE, postJson } from "./client";
import { fromFixture } from "./fixture";

/** Hand-written: the schema has no session route yet. */
export interface Session {
  /** The name the human signed in under; null before the name prompt has been answered. */
  humanName: string | null;
  /** False when the daemon needs a token and the request carries none. */
  authenticated: boolean;
}

/** A localhost daemon needs no token, so an empty answer asks only for the name. */
const UNNAMED: Session = { humanName: null, authenticated: true };

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/session")
  : undefined;

export async function getSession(): Promise<Session> {
  return fromFixture(fixture, (m) => m.session, UNNAMED);
}

/** The daemon answers with the `fold_human` cookie; the name is read back through `getSession`. */
export async function setName(
  name: string,
  fetchImpl?: typeof fetch,
): Promise<void> {
  await postJson(`${API_BASE}/session`, { name }, fetchImpl);
}

/** The daemon answers with the `fold_session` cookie. The token is sent once and kept nowhere. */
export async function signIn(
  token: string,
  fetchImpl?: typeof fetch,
): Promise<void> {
  await postJson(`${API_BASE}/session`, { token }, fetchImpl);
}
