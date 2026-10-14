import { fromFixture } from "./fixture";

export interface Session {
  humanName: string;
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/session")
  : undefined;

/** `null` when no human has signed in. */
export async function getSession(): Promise<Session | null> {
  return fromFixture(fixture, (m) => m.session, null);
}
