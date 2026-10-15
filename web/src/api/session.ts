import { API_BASE, postJson } from "./client";
import { fromFixture } from "./fixture";
import type { components } from "./schema";

export type Session = components["schemas"]["SessionStatus"];
type SessionRequest = components["schemas"]["SessionRequest"];

/** A localhost daemon needs no token, so an empty answer asks only for the name. */
const UNNAMED: Session = { human: null, authenticated: true };

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
  const body: SessionRequest = { name };
  await postJson(`${API_BASE}/session`, body, fetchImpl);
}

/** The daemon answers with the `fold_session` cookie. The token is sent once and kept nowhere. */
export async function signIn(
  token: string,
  fetchImpl?: typeof fetch,
): Promise<void> {
  const body: SessionRequest = { token };
  await postJson(`${API_BASE}/session`, body, fetchImpl);
}
