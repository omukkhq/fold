import { useEffect, useState } from "react";
import {
  readViewed,
  selectFile,
  showFile,
  toggleViewed,
  writeViewed,
  type MarkStore,
  type OpenMarks,
} from "./viewed";

function browserStore(): MarkStore | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/** The reader's state of one change's file tree. */
export interface ChangedFilesState {
  closed: ReadonlySet<string>;
  setClosed: (
    update: (old: ReadonlySet<string>) => ReadonlySet<string>,
  ) => void;
  filter: string;
  setFilter: (filter: string) => void;
  /** The files marked viewed in this browser. */
  viewed: ReadonlySet<string>;
  toggleViewed: (path: string) => void;
  /** The reader selects a file; null for one without a viewed mark. */
  selectFile: (path: string | null) => void;
}

/** The folders closed, the filter and the viewed marks of one change; the marks are kept in this browser under `viewedKey`. `shown` is the file whose diff the screen shows, which is marked viewed when it opens. It lives with the screen, so the tree keeps its folders and filter, and an open file stays open, while the screen shows something else in its place. */
export function useChangedFiles(
  viewedKey: string,
  shown: string | null,
): ChangedFilesState {
  const [closed, setClosed] = useState<ReadonlySet<string>>(new Set());
  const [filter, setFilter] = useState("");
  const [marks, setMarks] = useState<OpenMarks>(() => ({
    opened: null,
    viewed: readViewed(browserStore(), viewedKey),
  }));
  const next = showFile(marks, shown);
  if (next !== marks) setMarks(next);
  const viewed = next.viewed;
  useEffect(() => {
    writeViewed(browserStore(), viewedKey, viewed);
  }, [viewedKey, viewed]);
  return {
    closed,
    setClosed,
    filter,
    setFilter,
    viewed,
    toggleViewed: (path) => {
      setMarks((old) => ({ ...old, viewed: toggleViewed(old.viewed, path) }));
    },
    selectFile: (path) => {
      setMarks((old) => selectFile(old, path));
    },
  };
}
