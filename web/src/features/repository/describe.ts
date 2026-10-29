import type {
  FileView,
  RepositoryState,
  Revision,
  TreeEntry,
} from "../../api/repository";
import { shortCommit } from "../../format/git";
import { formatAsOfMinute } from "../../format/time";
import { actorSegment, seg, type Segments } from "@/people/segments";

export interface RevisionLines {
  title: string;
  /** What the revision is and who made it, such as `P-5 for T-15 by @frontend/forms`. */
  what: Segments;
  /** The local time it was made, to the minute; the row adds its Git commit after it. */
  when: string;
}

/** A revision in the shape of `fld log`: a proposal integration is titled by its proposal's title and names the proposal, its task and its author; a pull, a direct change of the main agent, a record of the project folder and the genesis say what they are and who made them, and fold's own record says fold made it. */
export function describeRevision(
  revision: Revision,
  now: number,
): RevisionLines {
  const when = formatAsOfMinute(now, revision.created_at);
  const by: Segments =
    revision.author === null ? [] : seg` by ${actorSegment(revision.author)}`;
  switch (revision.origin) {
    case "proposal": {
      const name = revision.proposal ?? "a proposal";
      return {
        title: revision.title ?? revision.proposal ?? revision.id,
        what:
          revision.task === null
            ? seg`${name}${by}`
            : seg`${name} for ${revision.task}${by}`,
        when,
      };
    }
    case "pull":
      return {
        title:
          revision.pull === null
            ? "Pull from a Git remote"
            : `Pull of the remote commit ${shortCommit(revision.pull)}`,
        what: seg`pulled from a Git remote${by}`,
        when,
      };
    case "direct":
      return {
        title: revision.title ?? revision.id,
        what: seg`a direct change on main${by}, approved by the user`,
        when,
      };
    case "record":
      // fold's own record, such as the one that stops tracking its skill files, changes main but records nothing from the folder.
      if (revision.author === "fold") {
        return {
          title: revision.title ?? revision.id,
          what: seg`a change of main made by fold`,
          when,
        };
      }
      return {
        title: revision.title ?? "Changes of the project folder",
        what: seg`recorded from the project folder${by}`,
        when,
      };
    case "initial":
      return {
        title: revision.title ?? "Genesis revision",
        what: seg`the start of main's history${by}`,
        when,
      };
  }
}

/** The daemon turns a name that is not UTF-8 into text with U+FFFD, and that text names no entry. Such an entry shows but is never read again by its path. */
export function isOpenable(entry: Pick<TreeEntry, "name" | "path">): boolean {
  return !entry.name.includes("\uFFFD") && !entry.path.includes("\uFFFD");
}

/** Whether the project folder holds an older revision than main's head: it waits on changes of the human's that no revision records, or on the Git branch. */
export function folderWaits(state: RepositoryState | undefined): boolean {
  return (
    state?.head != null && state.folder != null && state.folder !== state.head
  );
}

/** The note under the tree, from main's head, the project folder and the revision it holds as the repository route names them; null until that read arrives or while main has no revision. */
export function mainCopyText(
  state: RepositoryState | undefined,
): string | null {
  if (state?.head == null) {
    return null;
  }
  const folder = state.folder ?? state.head;
  if (folderWaits(state)) {
    return `The project folder ${state.main_path} shows ${folder}, apart from your changes that are not recorded, and waits before ${state.head}; Needs you says why. Agents read it; builds run in workspaces.`;
  }
  return `The project folder ${state.main_path} shows ${state.head}, apart from your changes that are not recorded. Agents read it; builds run in workspaces.`;
}

/** `→ ../shared/config.ts` beside a symlink in the tree, or null when the entry carries no target. The target is never followed. */
export function symlinkLabel(
  entry: Pick<TreeEntry, "kind" | "target">,
): string | null {
  return entry.kind === "symlink" && entry.target !== undefined
    ? `→ ${entry.target}`
    : null;
}

/** What the file view draws for a file. */
export type FileState =
  | { kind: "text"; text: string }
  | { kind: "symlink"; target: string }
  | { kind: "binary" }
  | { kind: "too_large" };

export function fileState(file: FileView): FileState {
  if (file.too_large) {
    return { kind: "too_large" };
  }
  if (file.binary) {
    return { kind: "binary" };
  }
  if (file.kind === "symlink") {
    return { kind: "symlink", target: file.text ?? "" };
  }
  return { kind: "text", text: file.text ?? "" };
}

/** `14.2 KB` style sizes for the tree and the binary notice. */
export function formatSize(bytes: number): string {
  if (bytes < 1024) {
    return `${String(bytes)} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
