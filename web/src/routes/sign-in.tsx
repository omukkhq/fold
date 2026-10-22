import { createFileRoute, redirect, useLocation } from "@tanstack/react-router";
import { sessionQuery } from "../api/queries";
import { linkSecret } from "../features/signin/link";
import { signInMode } from "../features/signin/mode";
import { safeRedirect } from "../features/signin/redirect";
import { SignInScreen } from "../features/signin/SignInScreen";

export const Route = createFileRoute("/sign-in")({
  validateSearch: (search: Record<string, unknown>) => ({
    redirect: typeof search.redirect === "string" ? search.redirect : undefined,
  }),
  beforeLoad: async ({ context, search, location }) => {
    if (linkSecret(location.hash) !== null) {
      return;
    }
    const session = await context.queryClient.query({
      ...sessionQuery(),
      staleTime: "static",
    });
    if (signInMode(session) === "ready") {
      redirect({
        href: safeRedirect(search.redirect),
        replace: true,
        throw: true,
      });
    }
  },
  component: SignInRoute,
});

function SignInRoute() {
  const { redirect: target } = Route.useSearch();
  const { hash } = useLocation();
  return <SignInScreen link={linkSecret(hash)} redirect={target} />;
}
