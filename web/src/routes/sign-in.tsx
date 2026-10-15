import { createFileRoute, redirect } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { sessionQuery } from "../api/queries";
import { signInMode } from "../features/signin/mode";
import { safeRedirect } from "../features/signin/redirect";
import { SignInScreen } from "../features/signin/SignInScreen";

export const Route = createFileRoute("/sign-in")({
  validateSearch: (search: Record<string, unknown>) => ({
    redirect: typeof search.redirect === "string" ? search.redirect : undefined,
  }),
  beforeLoad: async ({ context, search }) => {
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
  const session = useQuery(sessionQuery());
  const mode = session.data ? signInMode(session.data) : "name";
  return (
    <SignInScreen mode={mode === "ready" ? "name" : mode} redirect={target} />
  );
}
