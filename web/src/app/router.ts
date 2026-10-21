import type { QueryClient } from "@tanstack/react-query";
import { createRouter } from "@tanstack/react-router";
import { routeTree } from "../routeTree.gen";
import { NotFound } from "./NotFound";
import { RouteError, RoutePending } from "./routeStates";

export function createAppRouter(queryClient: QueryClient) {
  return createRouter({
    routeTree,
    context: { queryClient },
    defaultPendingComponent: RoutePending,
    defaultErrorComponent: RouteError,
    defaultNotFoundComponent: NotFound,
    defaultPendingMs: 150,
    defaultPendingMinMs: 250,
    // The screens scroll inside the shell's main, not the window, so a navigation resets main.
    scrollToTopSelectors: ["main"],
  });
}

export type AppRouter = ReturnType<typeof createAppRouter>;
