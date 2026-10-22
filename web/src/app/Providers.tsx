import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { api, authRequiredMiddleware } from "../api/client";
import { ThemeProvider } from "../theme/ThemeProvider";
import { redirectToSignIn } from "./authRedirect";
import { createQueryClient } from "./queryClient";
import { createAppRouter, type AppRouter } from "./router";

const queryClient = createQueryClient();
const router = createAppRouter(queryClient);

// Every route answers `auth_required` once the session ends; whichever screen asked is sent to sign in.
api.use(
  authRequiredMiddleware(() => {
    redirectToSignIn(queryClient, router.state.location, (href) => {
      void router.navigate({ href, replace: true });
    });
  }),
);

declare module "@tanstack/react-router" {
  interface Register {
    router: AppRouter;
  }
}

export function Providers() {
  return (
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </ThemeProvider>
  );
}
