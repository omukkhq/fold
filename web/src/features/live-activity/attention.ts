import type { AttentionItem, WorkspaceEntry } from "../../api/workspaces";
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

export function mainLine(
  entry: Pick<WorkspaceEntry, "base" | "main" | "behind">,
): string {
  if (entry.behind === 0) {
    return `Base ${entry.base} is current main.`;
  }
  const noun = entry.behind === 1 ? "revision" : "revisions";
  return `Main is at ${entry.main}; base ${entry.base} is ${String(entry.behind)} ${noun} behind.`;
}
