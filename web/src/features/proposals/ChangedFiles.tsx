import type { ReactNode } from "react";
import type { Intent } from "@/api/proposals";
import { Checkbox } from "@/components/form/controls";
import { DiffViewer, type DiffViewerProps } from "@/features/diff/DiffViewer";
import type { ChangedFilesState } from "./changedFilesState";
import { FileTree } from "./FileTree";
import { changeTree, folderPaths, toggleFolder, treeFiles } from "./folderTree";
import { partOf } from "./intent";
import { OUTSIDE_INTENT, groupFiles, partGroups } from "./partGroups";
import { shownPath, viewedText } from "./viewed";

export type ChangedFilesProps = Omit<
  DiffViewerProps,
  "fileNav" | "headExtra" | "headNote"
> & {
  state: ChangedFilesState;
  /** The intent of the shown version, whose parts group the files; null without one. */
  intent?: Intent | null;
  /** An action in the file's head before its viewed mark, such as a human's edit. */
  headAction?: (path: string) => ReactNode;
};

/** Above a file's lines: the part of the intent it belongs to and what that part does, or a warning when no part names it. */
function PartNote({
  part,
  path,
}: {
  part: ReturnType<typeof partOf>;
  path: string;
}) {
  if (part === null) {
    return (
      <p className="mx-14 mb-10 rounded-m bg-warn-soft px-12 py-10 text-14 text-text">
        <b className="font-title text-warn">{OUTSIDE_INTENT}:</b>{" "}
        <span className="font-mono text-13">{path}</span> belongs to no part of
        the intent.
      </p>
    );
  }
  return (
    <p className="mx-14 mb-10 rounded-m bg-accent-soft px-12 py-10 text-14 text-text">
      <b className="font-title">{part.name}:</b> {part.does}
    </p>
  );
}

/** The changed files of one comparison: the folder tree with its filter and viewed marks beside the diff viewer. Selecting a file in the tree marks it viewed. */
export function ChangedFiles({
  state,
  headAction,
  intent = null,
  ...viewer
}: ChangedFilesProps) {
  const tree = changeTree(viewer.comparison.files);
  const groups = partGroups(intent, viewer.comparison.files);
  const paths = (groups === null ? treeFiles(tree) : groupFiles(groups)).map(
    (f) => f.path,
  );
  return (
    <DiffViewer
      {...viewer}
      fileNav={
        <FileTree
          nodes={tree}
          groups={groups}
          selectedPath={viewer.selectedPath ?? null}
          viewed={state.viewed}
          viewedText={viewedText(state.viewed, paths)}
          closed={state.closed}
          filter={state.filter}
          onFilter={state.setFilter}
          onToggle={(folder) => {
            state.setClosed((old) => toggleFolder(old, folder));
          }}
          onExpandAll={() => {
            state.setClosed(() => new Set());
          }}
          onCollapseAll={() => {
            state.setClosed(() => new Set(folderPaths(tree)));
          }}
          onSelect={(path) => {
            state.selectFile(shownPath(viewer.comparison, path));
            viewer.onSelectFile(path);
          }}
        />
      }
      headNote={
        intent === null
          ? undefined
          : (path) => <PartNote part={partOf(intent, path)} path={path} />
      }
      headExtra={(path) => (
        <>
          {headAction?.(path)}
          <Checkbox
            label="Viewed"
            checked={state.viewed.has(path)}
            onCheckedChange={() => {
              state.toggleViewed(path);
            }}
          />
        </>
      )}
    />
  );
}
