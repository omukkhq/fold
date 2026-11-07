import { ApiError, apiErrorCode } from "@/api/errors";
import { commonText } from "@/format/failure";

/** The reason the screen gives when the stash it showed is not the stash the daemon holds; the daemon uses the same one. */
export const STASH_CHANGED = "stash_changed";

/** The dialog title once a drop or Clear all was refused because the stack moved. */
export const STASH_CHANGED_TITLE = "Nothing was dropped";

/** The text for a stash that changed between the list and the click. */
export const STASH_CHANGED_TEXT = "The stash changed; check the list again.";

/** A refusal of the screen itself, before any request, in the daemon's envelope so one function words both. */
export function stashChangedError(): ApiError {
  return new ApiError(
    "invalid_request",
    "the stash changed since it was listed",
    { reason: STASH_CHANGED },
  );
}

function detailsOf(error: unknown): Readonly<Record<string, unknown>> {
  return error instanceof ApiError ? error.details : {};
}

function pathsOf(error: unknown): string[] {
  const paths = detailsOf(error).paths;
  return Array.isArray(paths)
    ? paths.filter((path): path is string => typeof path === "string")
    : [];
}

function withPaths(text: string, paths: readonly string[]): string {
  return paths.length === 0
    ? text
    : `${text} ${paths.map((path) => `\`${path}\``).join(", ")}.`;
}

const REASONS: Readonly<Record<string, (paths: readonly string[]) => string>> =
  {
    [STASH_CHANGED]: () => STASH_CHANGED_TEXT,
    stash_entry_missing: () => STASH_CHANGED_TEXT,
    stash_empty: () => "The stash is empty now; nothing is dropped.",
    stash_conflicted: (paths) =>
      withPaths(
        "An unstash conflict waits in the project folder. Fix the files and run `fld record`, or reset the folder. Files:",
        paths,
      ),
    changed_during_stash: () =>
      "Files changed while fold stashed them. Nothing was put back; try again.",
    skipped_tracked: (paths) =>
      withPaths(
        "fold cannot stash a tracked file that is too large or unreadable. Make it smaller or readable, then try again. Files:",
        paths,
      ),
    head_moved: () =>
      "The Git branch or HEAD of the project folder shows a change fold did not make. Put the branch back first; fold never moves Git here.",
    git_commits_wait: () =>
      "Your Git commits wait for the main agent. Let the main agent take them first.",
    no_paths: () => "No file is named to restore.",
  };

/**
 * The sentence for a refused write to the project folder or its stash, or null when the generic banner words it
 * with the daemon's message. It branches on `code` and on `details.reason`, never on the message.
 */
export function folderErrorText(error: unknown): string | null {
  const code = apiErrorCode(error);
  if (code === "invalid_request") {
    const reason = detailsOf(error).reason;
    const text = typeof reason === "string" ? REASONS[reason] : undefined;
    return text === undefined ? null : text(pathsOf(error));
  }
  if (code === "human_required") {
    return "Only a signed-in human can change the project folder or its stash.";
  }
  return code === null ? null : (commonText(code) ?? null);
}

/** Whether a refusal means the stash on screen is not the daemon's any more, so the dialog's question no longer holds and only the way back to the list stays. */
export function stashChanged(error: unknown): boolean {
  if (apiErrorCode(error) !== "invalid_request") {
    return false;
  }
  const reason = detailsOf(error).reason;
  return reason === STASH_CHANGED || reason === "stash_entry_missing";
}
