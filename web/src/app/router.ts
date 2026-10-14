import type { QueryClient } from "@tanstack/react-query";
import { createRouter } from "@tanstack/react-router";
import { routeTree } from "../routeTree.gen";
import { NotFound } from "./NotFound";
import { RouteError } from "./RouteError";
import { RoutePending } from "./RoutePending";

export function createAppRouter(queryClient: QueryClient) {
  return createRouter({
    routeTree,
    context: { queryClient },
    defaultPendingComponent: RoutePending,
    defaultErrorComponent: RouteError,
    defaultNotFoundComponent: NotFound,
    defaultPendingMs: 150,
    defaultPendingMinMs: 250,
  });
}

export type AppRouter = ReturnType<typeof createAppRouter>;
