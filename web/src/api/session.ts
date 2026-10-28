import { api } from "./client";
import { expectOk, unwrap } from "./result";
import type { components } from "./schema";

export type Session = components["schemas"]["SessionStatus"];

export async function getSession(): Promise<Session> {
  return unwrap(api.GET("/api/v1/session"));
}

/** The daemon answers with the `fold_session` cookie. The token is sent once and kept nowhere. */
export async function signIn(token: string): Promise<void> {
  await expectOk(api.POST("/api/v1/session", { body: { token } }));
}

/** The daemon answers with the `fold_session` cookie and deletes the link, so it signs in once. */
export async function signInWithLink(link: string): Promise<void> {
  await expectOk(api.POST("/api/v1/session", { body: { link } }));
}

/** Deletes the session the `fold_session` cookie names, so the cookie stops working even where the browser keeps it. */
export async function signOut(): Promise<void> {
  await expectOk(api.DELETE("/api/v1/session"));
}
