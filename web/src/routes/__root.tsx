import type { QueryClient } from "@tanstack/react-query";
import {
  Outlet,
  createRootRouteWithContext,
  redirect,
} from "@tanstack/react-router";
import { sessionQuery } from "../api/queries";
import { signInMode } from "../features/signin/mode";
import { SIGN_IN_PATH, signInHref } from "../features/signin/redirect";

export const Route = createRootRouteWithContext<{ queryClient: QueryClient }>()(
  {
    beforeLoad: async ({ context, location }) => {
      if (location.pathname === SIGN_IN_PATH) {
        return;
      }
      const session = await context.queryClient.query({
        ...sessionQuery(),
        staleTime: "static",
      });
      if (signInMode(session) !== "ready") {
        // Untyped href: the sign-in route is generated after this file is typed.
        redirect({
          href: signInHref(location.href),
          replace: true,
          throw: true,
        });
      }
    },
    component: Outlet,
  },
);
