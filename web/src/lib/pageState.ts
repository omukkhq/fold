import { useCallback, useSyncExternalStore } from "react";

function mediaList(query: string): MediaQueryList | null {
  return typeof window.matchMedia === "function"
    ? window.matchMedia(query)
    : null;
}

/** Whether `query` matches now, kept current as the window changes. Without `matchMedia` it is false. */
export function useMediaQuery(query: string): boolean {
  const subscribe = useCallback(
    (change: () => void) => {
      const list = mediaList(query);
      list?.addEventListener("change", change);
      return () => {
        list?.removeEventListener("change", change);
      };
    },
    [query],
  );
  return useSyncExternalStore(
    subscribe,
    () => mediaList(query)?.matches ?? false,
    () => false,
  );
}

function subscribeVisibility(change: () => void) {
  document.addEventListener("visibilitychange", change);
  return () => {
    document.removeEventListener("visibilitychange", change);
  };
}

/** Whether the page's tab is in the foreground, kept current as the human switches tabs. */
export function usePageVisible(): boolean {
  return useSyncExternalStore(
    subscribeVisibility,
    () => document.visibilityState === "visible",
    () => true,
  );
}
