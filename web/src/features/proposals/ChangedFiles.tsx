import type { ReactNode } from "react";
import type { Intent } from "@/api/proposals";
import { Checkbox } from "@/components/form/controls";
import { DiffViewer, type DiffViewerProps } from "@/features/diff/DiffViewer";
import type { ChangedFilesState } from "./changedFilesState";
import { FileTree } from "./FileTree";
import { changeTree, folderPaths, toggleFolder, treeFiles } from "./folderTree";
import { MentionText } from "@/mentions/MentionText";
import {
  BACK_TO_BASE,
  OUTSIDE_INTENT,
  fileNote,
  groupFiles,
  partGroups,
  type FileNote,
} from "./partGroups";
import { shownPath, viewedText } from "./viewed";

export type ChangedFilesProps = Omit<
  DiffViewerProps,
  "fileNav" | "headExtra" | "headNote"
> & {
  slug: string;
  state: ChangedFilesState;
  /** The intent of the shown version, whose parts group the files; null without one. */
  intent?: Intent | null;
  /** The comparison is a change since an earlier version, whose files outside the parts the version changed back to the base. */
  since?: boolean;
  /** An action in the file's head before its viewed mark, such as a human's edit. */
  headAction?: (path: string) => ReactNode;
};

/** Above a file's lines: the part of the intent it belongs to and what that part does, a warning when no part names it, or that the version changed it back to the base. */
function PartNote({
  slug,
  note,
  path,
}: {
  slug: string;
  note: FileNote;
  path: string;
}) {
  if (note.kind === "outside") {
    return (
      <p className="mx-14 mb-10 rounded-m bg-warn-soft px-12 py-10 text-14 text-text">
        <b className="font-title text-warn">{OUTSIDE_INTENT}:</b>{" "}
        <span className="font-mono text-13">{path}</span> belongs to no part of
        the intent.
      </p>
    );
  }
  if (note.kind === "back") {
    return (
      <p className="mx-14 mb-10 rounded-m bg-sunken px-12 py-10 text-14 text-text">
        <b className="font-title">{BACK_TO_BASE}:</b> this version leaves{" "}
        <span className="font-mono text-13">{path}</span> as the base has it.
      </p>
    );
  }
  return (
    <div className="mx-14 mb-10 flex flex-col gap-2 rounded-m bg-accent-soft px-12 py-10 text-14 text-text">
      <b className="font-title">{note.part.name}:</b>
      <MentionText text={note.part.does} slug={slug} className="text-14" />
    </div>
  );
}

/** The changed files of one comparison: the folder tree with its filter and viewed marks beside the diff viewer. Selecting a file in the tree marks it viewed. */
export function ChangedFiles({
  slug,
  state,
  headAction,
  intent = null,
  since = false,
  ...viewer
}: ChangedFilesProps) {
  const tree = changeTree(viewer.comparison.files);
  const groups = partGroups(intent, viewer.comparison.files, since);
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
          : (path) => (
              <PartNote
                slug={slug}
                note={fileNote(intent, path, since)}
                path={path}
              />
            )
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
