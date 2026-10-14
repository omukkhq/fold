import type { QueryClient } from "@tanstack/react-query";
import { keys } from "../../api/keys";
import type { WorkspaceEntry } from "../../api/workspaces";
import { isLiveWorkspace, type LiveWorkspace } from "../../live/envelope";

export interface Panel {
  entry: WorkspaceEntry;
  /** The live state of an active workspace; null for a missing one, which has no file list. */
  live: LiveWorkspace | null;
}

/** Puts the live state of a listed workspace in the cache unless the stream already has it, so the stream stays the only writer after that. */
export function seedLive(
  queryClient: QueryClient,
  slug: string,
  entries: readonly WorkspaceEntry[],
): void {
  for (const entry of entries) {
    const key = keys.workspaceLive(slug, entry.workspace);
    if (entry.live !== null && queryClient.getQueryData(key) === undefined) {
      queryClient.setQueryData(key, entry.live);
    }
  }
}

/**
 * An active workspace shows only while the cache holds its live state, so a snapshot that drops it removes its panel.
 * `lives[i]` is the cached state of `entries[i]`.
 */
export function buildPanels(
  entries: readonly WorkspaceEntry[],
  lives: readonly unknown[],
): Panel[] {
  const panels: Panel[] = [];
  entries.forEach((entry, index) => {
    if (entry.state === "missing") {
      panels.push({ entry, live: null });
      return;
    }
    const live = lives[index];
    if (isLiveWorkspace(live)) {
      panels.push({ entry, live });
    }
  });
  return panels;
}

export function lastActivity(panel: Panel): number {
  return panel.live?.last_activity_at ?? panel.entry.last_activity_at;
}
