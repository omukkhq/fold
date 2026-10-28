import {
  useQuery,
  type QueryKey,
  type UseQueryOptions,
} from "@tanstack/react-query";
import type { ComparedFile, FileDiff } from "@/api/diff";
import type { FileView } from "@/api/repository";
import type { FileDiffView } from "./DiffViewer";
import type { FileContent, FileTextView } from "./gaps";
import { fileBody } from "./layout";

/** The part of a file's query the viewer reads. */
export interface FileDiffQuery {
  isError: boolean;
  data: FileDiff | undefined;
  refetch: () => unknown;
}

/** What the viewer draws for a file's query: its hunks, a retry after a failed read, or the pending state. */
export function fileDiffView(query: FileDiffQuery): FileDiffView {
  if (query.isError) {
    return {
      status: "error",
      retry: () => {
        void query.refetch();
      },
    };
  }
  return query.data
    ? { status: "ready", diff: query.data }
    : { status: "pending" };
}

/** The hunks of the file `options` reads, read only while `enabled`: a file whose body is not text needs none. */
export function useFileDiff<K extends QueryKey>(
  options: UseQueryOptions<FileDiff, Error, FileDiff, K>,
  enabled: boolean,
): FileDiffView {
  return fileDiffView(useQuery({ ...options, enabled }));
}

/** The part of a file read's query the viewer reads. */
export interface FileTextQuery {
  isError: boolean;
  data: FileContent | undefined;
  refetch: () => unknown;
}

/** What the viewer shows of a file read's query: its file, a retry after a failed read, or the pending state. */
export function fileTextView(query: FileTextQuery): FileTextView {
  if (query.isError) {
    return {
      status: "error",
      retry: () => {
        void query.refetch();
      },
    };
  }
  return query.data
    ? { status: "ready", file: query.data }
    : { status: "pending" };
}

/** Whether the shown file has unchanged lines around its hunks to read: a text diff with hunks of a file that is on both sides. */
export function needsFileText(
  file: ComparedFile | null,
  fileDiff: FileDiffView,
): boolean {
  if (file === null || fileBody(file) !== "text") return false;
  if (fileDiff.status !== "ready") return false;
  const diff = fileDiff.diff;
  return (
    !diff.binary &&
    !diff.too_large &&
    diff.hunks.length > 0 &&
    diff.old_hash !== null &&
    diff.new_hash !== null
  );
}

/** The new side of the file `options` reads, read only while `enabled`; undefined while not, so the viewer shows the hunks alone. */
export function useFileText<K extends QueryKey>(
  options: UseQueryOptions<FileView, Error, FileView, K>,
  enabled: boolean,
): FileTextView | undefined {
  const view = fileTextView(useQuery({ ...options, enabled }));
  return enabled ? view : undefined;
}
