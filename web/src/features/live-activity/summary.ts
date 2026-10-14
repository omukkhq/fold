import type { LiveWorkspace } from "../../live/envelope";

type Files = LiveWorkspace["files"];

/** The one-line change summary, `3 files, +42 -7`; a binary file counts as a file and adds no lines. */
export function summarize(files: Files): string {
  let added = 0;
  let removed = 0;
  for (const file of files) {
    added += file.added_lines;
    removed += file.removed_lines;
  }
  const noun = files.length === 1 ? "file" : "files";
  return `${String(files.length)} ${noun}, +${String(added)} -${String(removed)}`;
}
