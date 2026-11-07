import { apiErrorCode } from "@/api/errors";
import { commonText } from "@/format/failure";
import type { SharedState } from "../../api/repository";

/** The note under the tree about the ignored files workers read through links, and those they cannot see; null until the read arrives or when there is nothing to say. Names and counts only; the screen sets each path in the path face. */
export interface SharedNote {
  /** The shared paths, after `Shared read-only with workers:`; null when none is shared. */
  shared: string[] | null;
  /** The ignored paths no workspace holds, after `lead`, and how many more the daemon left out; null when workers miss nothing. */
  unseen: { lead: string; paths: string[]; more: number } | null;
  /** What to run to share one, shown only beside `unseen`. */
  hint: string | null;
}

/** A path as the note shows it: a folder ends with one `/`, as the daemon marks the folders it lists. */
export function pathForm(path: string): string {
  return path.replace(/\/+$/, "/");
}

/** The lines of the note for `state`, or null when nothing is shared and workers miss nothing. */
export function sharedNote(state: SharedState | undefined): SharedNote | null {
  if (state === undefined) {
    return null;
  }
  const shared = state.shared.length === 0 ? null : state.shared.map(pathForm);
  const unseen =
    state.unseen_count > 0
      ? {
          lead: `Workers cannot see ${String(state.unseen_count)} ${state.unseen_count === 1 ? "ignored path" : "ignored paths"}:`,
          paths: state.unseen.map(pathForm),
          more: Math.max(0, state.unseen_count - state.unseen.length),
        }
      : null;
  if (shared === null && unseen === null) {
    return null;
  }
  return {
    shared,
    unseen,
    hint: unseen === null ? null : "fld share <path>",
  };
}

/** Why the shared paths could not load, by the refusal's code; a path is marked between backticks for the path face. */
export function sharedFailure(error: unknown): string {
  const code = apiErrorCode(error);
  if (code === null) return "The daemon did not answer.";
  switch (code) {
    case "workspace_io":
      return "fold could not read `.fold/shared` or the ignored files of the project folder.";
    case "internal_error":
      return "The daemon failed. Try again.";
    default:
      return commonText(code) ?? "The daemon refused the request.";
  }
}
