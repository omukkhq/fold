import { createContext, useContext } from "react";

/** Starts the open project's next connection attempt now instead of at the end of the backoff delay. */
export const LiveRetryContext = createContext<() => void>(() => undefined);

export function useLiveRetry(): () => void {
  return useContext(LiveRetryContext);
}
