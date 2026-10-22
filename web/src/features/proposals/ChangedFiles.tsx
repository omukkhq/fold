import { Checkbox } from "@/components/form/controls";
import { DiffViewer, type DiffViewerProps } from "@/features/diff/DiffViewer";
import type { ChangedFilesState } from "./changedFilesState";
import { FileTree } from "./FileTree";
import { changeTree, folderPaths, toggleFolder, treeFiles } from "./folderTree";
import { viewedText } from "./viewed";

export type ChangedFilesProps = Omit<
  DiffViewerProps,
  "fileNav" | "headExtra"
> & {
  state: ChangedFilesState;
};

/** The changed files of one comparison: the folder tree with its filter and viewed marks beside the diff viewer. */
export function ChangedFiles({ state, ...viewer }: ChangedFilesProps) {
  const tree = changeTree(viewer.comparison.files);
  const paths = treeFiles(tree).map((f) => f.path);
  return (
    <DiffViewer
      {...viewer}
      fileNav={
        <FileTree
          nodes={tree}
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
          onSelect={viewer.onSelectFile}
        />
      }
      headExtra={(path) => (
        <Checkbox
          label="Viewed"
          checked={state.viewed.has(path)}
          onCheckedChange={() => {
            state.toggleViewed(path);
          }}
        />
      )}
    />
  );
}
