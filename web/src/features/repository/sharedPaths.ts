import type { SharedState } from "../../api/repository";

/** The note under the tree about the ignored files workers read through links, and those they cannot see; null until the read arrives or when there is nothing to say. Names and counts only. */
export interface SharedNote {
  /** The shared paths, such as `Shared read-only with workers: design/, docs/spec.pdf.` */
  shared: string | null;
  /** The ignored paths no workspace holds, such as `Workers cannot see 3 ignored paths: .env, notes.md and 1 more.` */
  unseen: string | null;
  /** What to run to share one, shown only beside `unseen`. */
  hint: string | null;
}

/** The lines of the note for `state`, or null when nothing is shared and workers miss nothing. */
export function sharedNote(state: SharedState | undefined): SharedNote | null {
  if (state === undefined) {
    return null;
  }
  const shared =
    state.shared.length === 0
      ? null
      : `Shared read-only with workers: ${state.shared.join(", ")}.`;
  let unseen: string | null = null;
  if (state.unseen_count > 0) {
    const noun = state.unseen_count === 1 ? "ignored path" : "ignored paths";
    const more = state.unseen_count - state.unseen.length;
    const tail = more > 0 ? ` and ${String(more)} more` : "";
    unseen = `Workers cannot see ${String(state.unseen_count)} ${noun}: ${state.unseen.join(", ")}${tail}.`;
  }
  if (shared === null && unseen === null) {
    return null;
  }
  return {
    shared,
    unseen,
    hint: unseen === null ? null : "fld share <path>",
  };
}
