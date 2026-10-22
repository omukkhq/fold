import { useState } from "react";
import {
  readViewed,
  toggleViewed,
  writeViewed,
  type MarkStore,
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
}

/** The folders closed, the filter and the viewed marks of one change; the marks are kept in this browser under `viewedKey`. It lives with the screen, so the tree keeps its folders and filter while the screen shows something else in its place. */
export function useChangedFiles(viewedKey: string): ChangedFilesState {
  const [closed, setClosed] = useState<ReadonlySet<string>>(new Set());
  const [filter, setFilter] = useState("");
  const [viewed, setViewed] = useState(() =>
    readViewed(browserStore(), viewedKey),
  );
  return {
    closed,
    setClosed,
    filter,
    setFilter,
    viewed,
    toggleViewed: (path) => {
      const next = toggleViewed(viewed, path);
      writeViewed(browserStore(), viewedKey, next);
      setViewed(next);
    },
  };
}
