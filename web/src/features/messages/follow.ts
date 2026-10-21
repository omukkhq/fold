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
