import type { AttentionItem } from "../../api/workspaces";
import { revisionCount, revisionsAhead } from "../../format/revisions";
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

/** Where the base stands against main; the base alone until main has been read. */
export function mainLine(
  base: string,
  repository:
    | {
        head: string | null;
        recent: readonly { id: string; parent: string | null }[];
      }
    | undefined,
): string {
  const head = repository?.head ?? null;
  if (repository === undefined || head === null) {
    return `Base ${base}.`;
  }
  if (head === base) {
    return `Base ${base} is current main.`;
  }
  const behind = revisionCount(revisionsAhead(repository.recent, base));
  return `Main is at ${head}; base ${base} is ${behind} behind.`;
}
