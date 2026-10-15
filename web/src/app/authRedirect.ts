import type { QueryClient } from "@tanstack/react-query";
import { keys } from "../api/keys";
import { SIGN_IN_PATH, signInHref } from "../features/signin/redirect";

interface Place {
  pathname: string;
  href: string;
}

/**
 * Sends the person to the sign-in screen with the current place as the way back. The cached session is dropped first: it still says authenticated, and the sign-in route would send the person straight back.
 * On the sign-in screen itself nothing happens.
 */
export function redirectToSignIn(
  queryClient: QueryClient,
  place: Place,
  navigate: (href: string) => void,
): void {
  if (place.pathname === SIGN_IN_PATH) {
    return;
  }
  queryClient.removeQueries({ queryKey: keys.session() });
  navigate(signInHref(place.href));
}
