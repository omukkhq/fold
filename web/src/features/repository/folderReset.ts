import type { NeedsYouItem } from "@/api/workers";

/** What the project folder waits on that a reset or a restore ends, from Needs you. */
export interface FolderActions {
  /** The conflicted paths of the human's change that waits for the main agent; null when none waits. */
  heldChange: string[] | null;
  /** The paths main changed that hold unrecorded changes of the human's, which a restore puts back. */
  heldPaths: string[];
  /** The paths where the conflict markers of an unstash wait. */
  markers: string[];
}

/** The folder's actions; null while nothing in Needs you waits on the project folder. */
export function folderActions(
  items: readonly NeedsYouItem[],
): FolderActions | null {
  const change = items.find((item) => item.kind === "human_change");
  const held = items.find((item) => item.kind === "folder_held");
  const markers = items.find((item) => item.kind === "unstash_conflicted");
  if (!change && !held && !markers) {
    return null;
  }
  return {
    heldChange: change ? change.paths : null,
    heldPaths: held?.paths ?? [],
    markers: markers?.paths ?? [],
  };
}

function filesText(count: number): string {
  return count === 1 ? "1 file" : `${String(count)} files`;
}

const SAFETY =
  "fold first stashes your unrecorded changes as stash@{0}, so Unstash brings them back.";

/** The reset button's label, and its dialog's title and words. */
export function resetText(actions: FolderActions): {
  label: string;
  title: string;
  description: string;
} {
  const parts = [SAFETY, "Then the project folder matches main again."];
  if (actions.heldChange !== null) {
    parts.push(
      `Your change that waits for the main agent is dropped, with its conflict in ${filesText(actions.heldChange.length)}, and fold abandons the merge workspace the main agent made for it.`,
    );
  }
  if (actions.markers.length > 0) {
    parts.push(
      `The unstash conflict in ${filesText(actions.markers.length)} ends; stash@{0} keeps those files with their markers.`,
    );
  }
  return actions.heldChange === null
    ? {
        label: "Reset folder",
        title: "Reset the project folder?",
        description: parts.join(" "),
      }
    : {
        label: "Drop my change",
        title: "Drop your waiting change?",
        description: parts.join(" "),
      };
}

/** The restore dialog's title and words for the held paths. */
export function restoreText(paths: readonly string[]): {
  title: string;
  description: string;
} {
  return {
    title: `Restore ${filesText(paths.length)}?`,
    description: `${SAFETY.replace("your unrecorded changes", "their changes")} Then these files match main again; your other changes stay.`,
  };
}
