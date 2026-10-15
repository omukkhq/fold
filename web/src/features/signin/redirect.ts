export const SIGN_IN_PATH = "/sign-in";
export const DEFAULT_LANDING = "/projects";

/**
 * The path to return to after sign-in, or the projects screen for anything else. Only a same-origin path that starts with a single `/` passes:
 * `//host` and `/\host` are network-path references, and a browser drops tabs and newlines inside a URL, so `/\t/host` is one too.
 */
export function safeRedirect(target: string | null | undefined): string {
  if (typeof target !== "string" || !target.startsWith("/")) {
    return DEFAULT_LANDING;
  }
  if (target.startsWith("//") || /[\\\p{Cc}]/u.test(target)) {
    return DEFAULT_LANDING;
  }
  const [pathname] = target.split(/[?#]/);
  if (pathname === SIGN_IN_PATH) {
    return DEFAULT_LANDING;
  }
  return target;
}

/** The sign-in location that returns to `target` afterwards. */
export function signInHref(target: string): string {
  return `${SIGN_IN_PATH}?redirect=${encodeURIComponent(target)}`;
}
