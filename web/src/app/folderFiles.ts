import type { ComparedFile } from "@/api/diff";
import { apiErrorCode } from "@/api/errors";
import type { SharedFile } from "@/api/folderDiff";
import { changeNote, lineCounts } from "@/features/diff/present";
import { formatSize } from "@/features/repository/describe";
import { commonText } from "@/format/failure";
import { formatAsOfMinute } from "@/format/time";

/** A file a Needs you item or a record names, with its change of the project folder; `file` is null once the folder holds the revision's version of it again. */
export interface FlaggedFile {
  path: string;
  file: ComparedFile | null;
}

/** The named files in the order given, each with its change from the folder's comparison. */
export function flaggedFiles(
  files: readonly ComparedFile[],
  paths: readonly string[],
): FlaggedFile[] {
  const byPath = new Map(files.map((file) => [file.path, file]));
  return paths.map((path) => ({ path, file: byPath.get(path) ?? null }));
}

/** The words beside a flagged file's path before it opens: its change and its line counts, such as `new file · +3`. */
export function flaggedFacts(flagged: FlaggedFile): string {
  if (flagged.file === null) return "holds the revision's version again";
  const { added, removed } = lineCounts(flagged.file);
  const parts = [
    flagged.file.binary ? "binary" : null,
    changeNote(flagged.file),
    flagged.file.change === "modified" &&
    added === null &&
    removed === null &&
    !flagged.file.binary
      ? "changed"
      : null,
    added,
    removed,
  ].filter((part): part is string => part !== null);
  return parts.join(" · ");
}

/** A changed shared file as the folder holds it now, such as `12.0 KB · changed 14:12`; ignored files have no history, so this stands for a diff. */
export function sharedFileFacts(file: SharedFile, now: number): string {
  if (file.kind === "missing") return "gone from the project folder";
  const kind =
    file.kind === "folder"
      ? "folder"
      : file.kind === "symlink"
        ? `symbolic link, ${formatSize(file.size ?? 0)}`
        : formatSize(file.size ?? 0);
  return file.modified === null
    ? kind
    : `${kind} · changed ${formatAsOfMinute(now, file.modified)}`;
}

/** Why the folder's diff could not load, by the refusal's code. */
export function folderFailure(error: unknown): string {
  const code = apiErrorCode(error);
  if (code === null) return "The daemon did not answer.";
  switch (code) {
    case "workspace_io":
      return "fold could not read a file of the project folder.";
    case "path_not_in_comparison":
      return "The file holds the revision's version again.";
    case "internal_error":
      return "The daemon failed. Try again.";
    default:
      return commonText(code) ?? "The daemon refused the request.";
  }
}
