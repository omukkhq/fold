import type {
  FileView,
  RepositoryState,
  Revision,
  TreeEntry,
} from "../../api/repository";
import { formatAsOf } from "../../format/time";
import {
  actorSegment,
  seg,
  type Segment,
  type Segments,
} from "@/people/segments";

export interface RevisionLines {
  title: string;
  /** Mono meta line: ids, who, and the time of day. */
  meta: Segments;
}

/** What a revision row shows: a proposal integration names its proposal and task, a pull names the remote commit and the human, a record names the human who changed the project folder, the genesis is the start of the history, and a direct change of the main agent names its commit subject and the user's approval. */
export function describeRevision(
  revision: Revision,
  now: number,
): RevisionLines {
  const at = formatAsOf(now, revision.created_at);
  const who: Segment =
    revision.author === null ? "unknown" : actorSegment(revision.author);
  switch (revision.origin) {
    case "proposal":
      return {
        title: [revision.proposal ?? revision.id, revision.title]
          .filter((part) => part !== null)
          .join(" · "),
        meta:
          revision.task === null
            ? seg`${who} · ${at}`
            : seg`${revision.task} · ${who} · ${at}`,
      };
    case "pull":
      return {
        title: `pull · ${revision.pull ?? ""}`,
        meta: seg`${who} · ${at}`,
      };
    case "direct":
      return {
        title: `direct · ${revision.title ?? revision.id}`,
        meta: seg`${who} · approved by the user · ${at}`,
      };
    case "record":
      return {
        title: `record · ${revision.title ?? "changes of the project folder"}`,
        meta: seg`${who} · ${at}`,
      };
    case "initial":
      return {
        title: revision.title ?? "Genesis revision",
        meta: seg`${who} · ${at}`,
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
