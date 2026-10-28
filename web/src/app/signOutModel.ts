import type { QueryClient } from "@tanstack/react-query";
import { keys } from "../api/keys";
import { SIGN_IN_PATH } from "../features/signin/redirect";

/**
 * After the daemon deleted the session: the cached session goes first, because it still says signed in and the sign-in route would send the person straight back; once the sign-in screen shows, the rest of the cache goes too, so the next person to sign in sees nothing of this one.
 */
export async function leaveAfterSignOut(
  queryClient: QueryClient,
  navigate: (href: string) => Promise<void>,
): Promise<void> {
  queryClient.removeQueries({ queryKey: keys.session() });
  await navigate(SIGN_IN_PATH);
  queryClient.clear();
}

/** Two letters for the avatar from the bare name, such as `PP` for `pptx704` or `TA` for `tanim-a`; null without a name. */
export function initials(name: string | null): string | null {
  if (name === null) return null;
  const parts = name.split(/[^\p{L}\p{N}]+/u).filter((p) => p !== "");
  const [first = "", second = ""] = parts;
  const letters =
    parts.length > 1
      ? `${first.slice(0, 1)}${second.slice(0, 1)}`
      : first.slice(0, 2);
  return letters === "" ? null : letters.toUpperCase();
}
