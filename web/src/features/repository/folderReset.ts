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
  "fold first stashes your unrecorded changes as stash@{0}, so fld unstash brings them back.";

const RESTORE_SAFETY =
  "fold first stashes the changes of these files as stash@{0}, so fld unstash brings them back.";

/** The reset button's label, and its dialog's title and words. */
export function resetText(actions: FolderActions): {
  label: string;
  title: string;
  description: string;
} {
  const parts = [SAFETY, "Then the project folder matches main again."];
  if (actions.heldChange !== null) {
    parts.push(
      `Your change that waits for the main agent is withdrawn: its files, with the conflict in ${filesText(actions.heldChange.length)}, go into stash@{0} too. If the main agent opened a workspace to merge it, fold closes that workspace.`,
    );
  }
  if (actions.markers.length > 0) {
    parts.push(
      `The unstash conflict in ${filesText(actions.markers.length)} ends: fold drops the stash entry that conflicted, and stash@{0} keeps its files as they are now, with the conflict markers.`,
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

/** The most paths a reset or restore dialog names before it sums up the rest. */
export const LISTED_DIALOG_PATHS = 9;

/** The line after the named paths of a reset, which stashes every unrecorded change of the folder while the web knows only the paths Needs you names. */
export const OTHER_CHANGES = "and your other unrecorded changes";

/** The paths a dialog lists and the line after them: a reset names the files of the waiting change, else the held files, then the other unrecorded changes it stashes too; a restore names the held files it stashes, then how many more. */
export function dialogPaths(
  action: "reset" | "restore",
  actions: FolderActions,
): { listed: string[]; rest: string | null } {
  const paths =
    action === "reset"
      ? (actions.heldChange ?? actions.heldPaths)
      : actions.heldPaths;
  const listed = paths.slice(0, LISTED_DIALOG_PATHS);
  if (action === "reset") {
    return { listed, rest: OTHER_CHANGES };
  }
  const more = paths.length - listed.length;
  return {
    listed,
    rest:
      more > 0
        ? `and ${String(more)} other ${more === 1 ? "file" : "files"}`
        : null,
  };
}

/** The restore dialog's title and words for the held paths. */
export function restoreText(paths: readonly string[]): {
  title: string;
  description: string;
} {
  return {
    title: `Restore ${filesText(paths.length)}?`,
    description: `${RESTORE_SAFETY} Then these files match main again; your other changes stay.`,
  };
}
