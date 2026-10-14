import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { ThemeProvider } from "../theme/ThemeProvider";
import { createQueryClient } from "./queryClient";
import { createAppRouter, type AppRouter } from "./router";

const queryClient = createQueryClient();
const router = createAppRouter(queryClient);

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
