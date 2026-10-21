/** How far from the bottom, in pixels, a reader still counts as following the newest message. */
export const FOLLOW_SLACK = 48;

export interface ScrollMetrics {
  scrollTop: number;
  clientHeight: number;
  scrollHeight: number;
}

/** True when the scroll position is at the bottom or within the slack of it. */
export function isFollowing(metrics: ScrollMetrics): boolean {
  return (
    metrics.scrollHeight - metrics.scrollTop - metrics.clientHeight <=
    FOLLOW_SLACK
  );
}

/** Puts the newest message in view. */
export function toBottom(box: ScrollMetrics): void {
  box.scrollTop = box.scrollHeight;
}

/** A reader who sent a message follows the thread again, so their own message comes into view. */
export function followNewest(
  box: ScrollMetrics | null,
  following: { current: boolean },
): void {
  following.current = true;
  if (box) {
    toBottom(box);
  }
}

/** The part of `ResizeObserver` that following uses, so a test can drive it. */
export type SizeObserver<T> = new (
  callback: () => void,
) => { observe(target: T): void; disconnect(): void };

/** The browser's `ResizeObserver`, which jsdom and older browsers lack although the DOM types declare it. */
export const browserSizeObserver = globalThis.ResizeObserver as
  | SizeObserver<Element>
  | undefined;

/**
 * Goes back to the newest message whenever one of the targets changes size while the reader follows, and returns the cleanup.
 * A thread mounted while hidden has no height and reaches its bottom only once it is shown; a resized window, a growing reply box and a longer delivery line move the bottom too.
 * A hidden box loses its scroll position, so it follows again when it is shown.
 */
export function followResizes<T>(
  box: ScrollMetrics,
  targets: readonly T[],
  following: { current: boolean },
  Observer: SizeObserver<T> | undefined,
): () => void {
  if (Observer === undefined) {
    return () => {};
  }
  const observer = new Observer(() => {
    if (box.clientHeight === 0) {
      following.current = true;
    } else if (following.current) {
      toBottom(box);
    }
  });
  for (const target of targets) {
    observer.observe(target);
  }
  return () => {
    observer.disconnect();
  };
}
