import { formatSize } from "./present";

export const MAX_FILES = 10;
export const MAX_FILE_BYTES = 10 * 1024 * 1024;
const MAX_NAME_BYTES = 255;

export interface FileFacts {
  name: string;
  size: number;
}

function hasControl(name: string): boolean {
  for (const char of name) {
    const code = char.codePointAt(0) ?? 0;
    if (code < 0x20 || (code >= 0x7f && code <= 0x9f)) return true;
  }
  return false;
}

/** The daemon's name rule: 1 to 255 bytes, no `/`, no `..`, no control character. */
export function validName(name: string): boolean {
  const bytes = new TextEncoder().encode(name).length;
  return (
    bytes > 0 &&
    bytes <= MAX_NAME_BYTES &&
    !name.includes("/") &&
    !name.includes("..") &&
    !hasControl(name)
  );
}

/** Why one dropped file cannot go, or null when it can. */
export function fileProblem(file: FileFacts): string | null {
  if (file.size > MAX_FILE_BYTES) {
    return `${file.name} is ${formatSize(file.size)}. Each file can be up to 10 MiB. Remove it or attach a smaller file.`;
  }
  if (!validName(file.name)) {
    return "A file name is empty, too long or holds characters fold does not accept. Rename the file.";
  }
  return null;
}

/** Adds dropped files up to the limit; the files past it are counted so the dialog can say how many it left out. */
export function addFiles<T extends FileFacts>(
  current: readonly T[],
  added: readonly T[],
): { files: T[]; refused: number } {
  const room = Math.max(0, MAX_FILES - current.length);
  return {
    files: [...current, ...added.slice(0, room)],
    refused: Math.max(0, added.length - room),
  };
}

/** The first problem that stops the upload, checked before anything is sent. */
export function blockingProblem(files: readonly FileFacts[]): string | null {
  if (files.length > MAX_FILES)
    return `A request holds at most ${String(MAX_FILES)} files.`;
  for (const file of files) {
    const problem = fileProblem(file);
    if (problem) return problem;
  }
  return null;
}

export function refusedText(refused: number): string | null {
  if (refused === 0) return null;
  return `${String(refused)} ${refused === 1 ? "file was" : "files were"} left out: a request holds at most ${String(MAX_FILES)} files.`;
}
