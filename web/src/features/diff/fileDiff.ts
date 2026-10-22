import {
  useQuery,
  type QueryKey,
  type UseQueryOptions,
} from "@tanstack/react-query";
import type { FileDiff } from "@/api/diff";
import type { FileDiffView } from "./DiffViewer";

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
