import { api } from "./client";
import { expectOk, unwrap } from "./result";
import type { components } from "./schema";

export type Session = components["schemas"]["SessionStatus"];

export async function getSession(): Promise<Session> {
  return unwrap(api.GET("/api/v1/session"));
}

/** The daemon answers with the `fold_human` cookie; the name is read back through `getSession`. */
export async function setName(name: string): Promise<void> {
  await expectOk(api.POST("/api/v1/session", { body: { name } }));
}

/** The daemon answers with the `fold_session` cookie. The token is sent once and kept nowhere. */
export async function signIn(token: string): Promise<void> {
  await expectOk(api.POST("/api/v1/session", { body: { token } }));
}
