import { QueryClient } from "@tanstack/react-query";

/** `fetch` rejects with a TypeError when the network fails; an answer from the daemon is never retried. */
export function shouldRetry(failureCount: number, error: unknown): boolean {
  return failureCount < 1 && error instanceof TypeError;
}

/** The event stream keeps cached data fresh, so nothing polls and nothing refetches on focus. */
export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        refetchOnWindowFocus: false,
        refetchInterval: false,
        retry: shouldRetry,
      },
    },
  });
}
