import type { LiveWorkspace } from "../../live/envelope";
import { checkpointHref } from "./links";

export interface RecordedItem {
  kind: "checkpoint";
  id: string;
  text: string;
  /** UTC milliseconds. */
  at: number;
  href: string;
}

/** Checkpoints of one workspace, newest first, each with the link to its comparison. */
export function recordedItems(
  slug: string,
  base: string,
  checkpoints: LiveWorkspace["checkpoints"],
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
  ];
  return items.sort((a, b) => b.at - a.at || a.id.localeCompare(b.id));
}
