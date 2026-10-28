import type { AttentionItem } from "../../api/workspaces";
import { revisionsAhead } from "../../format/revisions";
import { formatQuiet } from "../../format/time";

export interface AttentionView {
  /** `no edits for 7 min` when the daemon reports the workspace stalled. */
  stalled: string | null;
  staleClaim: boolean;
}

/**
 * Shows the daemon's `stalled` and `stale_claim` items and ignores the kinds this screen does not draw; the web decides nothing.
 * `lastActivityAt` is the workspace's last edit, which is what a stalled workspace has been quiet since.
 */
export function attentionView(
  items: readonly AttentionItem[],
  lastActivityAt: number,
  now: number,
): AttentionView {
  return {
    stalled: items.some((item) => item.kind === "stalled")
      ? formatQuiet(now, lastActivityAt)
      : null,
    staleClaim: items.some((item) => item.kind === "stale_claim"),
  };
}

interface MainState {
  head: string | null;
  recent: readonly { id: string; parent: string | null }[];
}

/** `Main is at r9`, said once for every workspace; null until main has been read or while it has no revision. */
export function mainHeadText(repository: MainState | undefined): string | null {
  const head = repository?.head ?? null;
  return head === null ? null : `Main is at ${head}`;
}

/** `2 behind` beside a base that main has moved past, `3+ behind` when the base is older than the revisions main lists; null for a base that is main, and until main has been read. */
export function behindTag(
  base: string,
  repository: MainState | undefined,
): string | null {
  const head = repository?.head ?? null;
  if (repository === undefined || head === null || head === base) {
    return null;
  }
  const ahead = revisionsAhead(repository.recent, base);
  return `${String(ahead.count)}${ahead.atLeast ? "+" : ""} behind`;
}
