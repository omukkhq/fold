/** A link secret is URL-safe base64, as the daemon writes it. */
const SECRET = /^[A-Za-z0-9_-]+$/;

/**
 * The secret of a one-time sign-in link from `fld web`, read from the location's fragment (`#link=…`), with or without its `#`.
 * The secret is in the fragment because a browser never sends a fragment to a server. Anything else is `null`.
 */
export function linkSecret(hash: string): string | null {
  const fragment = hash.startsWith("#") ? hash.slice(1) : hash;
  const secret = new URLSearchParams(fragment).get("link");
  return secret !== null && SECRET.test(secret) ? secret : null;
}

/** `href` without its fragment, so the address bar and the history keep no copy of a used link. */
export function withoutFragment(href: string): string {
  const at = href.indexOf("#");
  return at === -1 ? href : href.slice(0, at);
}

/** What the sign-in screen shows: the link it opens, or the token form once there is no link or the person chose a token. */
export type SignInView = { kind: "link"; secret: string } | { kind: "token" };

export function signInView(
  secret: string | null,
  choseToken: boolean,
): SignInView {
  return secret === null || choseToken
    ? { kind: "token" }
    : { kind: "link", secret };
}
