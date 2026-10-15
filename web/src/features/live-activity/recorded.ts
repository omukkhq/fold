import type { ChangesetSummary } from "../../api/workspaces";
import type { LiveWorkspace } from "../../live/envelope";
import { changesetHref, checkpointHref } from "./links";

export interface RecordedItem {
  kind: "checkpoint" | "changeset";
  id: string;
  text: string;
  /** UTC milliseconds. */
  at: number;
  href: string;
}

/** Checkpoints and changesets of one workspace, newest first, each with the link to its comparison. No route sends changesets yet, so the list holds them only when a workspace read carries them. */
export function recordedItems(
  slug: string,
  base: string,
  checkpoints: LiveWorkspace["checkpoints"],
  changesets: readonly ChangesetSummary[] = [],
): RecordedItem[] {
  const items: RecordedItem[] = [
    ...checkpoints.map((checkpoint) => ({
      kind: "checkpoint" as const,
      id: checkpoint.id,
      text: [
        "checkpoint",
        checkpoint.automatic ? "automatic" : "explicit",
        checkpoint.label,
      ]
        .filter((part) => part !== null)
        .join(" · "),
      at: checkpoint.created_at,
      href: checkpointHref(slug, base, checkpoint.id),
    })),
    ...changesets.map((changeset) => ({
      kind: "changeset" as const,
      id: changeset.id,
      text: changeset.message,
      at: changeset.recorded_at,
      href: changesetHref(slug, changeset.base, changeset.id),
    })),
  ];
  return items.sort((a, b) => b.at - a.at || a.id.localeCompare(b.id));
}
