import { ChevronsDown, TriangleAlert } from "lucide-react";
import { useState, type ReactNode } from "react";
import type { ComparedFile, FileDiff, FileWarning } from "@/api/diff";
import { Button } from "@/components/button/Button";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { DiffRows } from "./DiffRows";
import { FileList } from "./FileList";
import {
  ROW_LIMIT,
  fileBody,
  layoutFile,
  listedFiles,
  shownEntry,
  threadEnds,
} from "./layout";
import {
  binaryNote,
  changeNote,
  lineCounts,
  moreRowsText,
  tooLargeNote,
  warningNote,
} from "./present";
import type { DiffSelection, DiffThread } from "./selection";

/** The selected file's hunks as its query holds them. */
export type FileDiffView =
  | { status: "pending" }
  | { status: "error"; retry: () => void }
  | { status: "ready"; diff: FileDiff };

/** The part of a comparison the viewer draws: the changed files and the working files that could not be read. */
export interface DiffFiles {
  files: readonly ComparedFile[];
  warnings: readonly FileWarning[];
}

export interface DiffViewerProps {
  comparison: DiffFiles;
  selectedPath: string | undefined;
  onSelectFile: (path: string) => void;
  /** The overline above the file list, when the screen draws one. */
  filesTitle?: string;
  /** Read only for a file whose body is text; a binary file and an unchanged rename need no hunks. */
  fileDiff: FileDiffView;
  selection: DiffSelection | null;
  onSelect: (selection: DiffSelection | null) => void;
  threads?: readonly DiffThread[];
  renderThread?: (thread: DiffThread) => ReactNode;
  /** Shown under the last selected line. */
  renderComposer?: (selection: DiffSelection) => ReactNode;
  /** Ranges marked at each of their lines, such as questions to the author. */
  marks?: readonly DiffThread[];
  renderMark?: (mark: DiffThread, first: boolean) => ReactNode;
  /** The rows a file shows before "Show more". */
  rowLimit?: number;
}

type BodyProps = Omit<
  DiffViewerProps,
  "comparison" | "selectedPath" | "onSelectFile" | "filesTitle"
> & { file: ComparedFile };

function Hash({ side, hash }: { side: string; hash: string | null }) {
  return (
    <div className="flex gap-10">
      <dt className="w-diff-number shrink-0 text-bar text-text-muted">
        {side}
      </dt>
      <dd className="min-w-0 font-mono text-code break-all text-text">
        {hash ?? "no file"}
      </dd>
    </div>
  );
}

function HashOnly({
  note,
  oldHash,
  newHash,
}: {
  note: string;
  oldHash: string | null;
  newHash: string | null;
}) {
  return (
    <div className="flex flex-col gap-8 px-14 py-12">
      <p className="text-bar text-text-muted">{note}</p>
      <dl className="flex flex-col gap-4">
        <Hash side="old" hash={oldHash} />
        <Hash side="new" hash={newHash} />
      </dl>
    </div>
  );
}

function FileBody({
  file,
  fileDiff,
  rowLimit = ROW_LIMIT,
  ...rows
}: BodyProps) {
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [limit, setLimit] = useState(rowLimit);
  const body = fileBody(file);

  if (body === "binary") {
    return (
      <HashOnly
        note={binaryNote(file.change)}
        oldHash={file.old_hash}
        newHash={file.new_hash}
      />
    );
  }
  if (body === "unchanged") {
    return (
      <p className="px-14 py-12 text-bar text-text-muted">
        The content did not change.
      </p>
    );
  }
  if (fileDiff.status === "pending") {
    return <Skeleton className="m-12" />;
  }
  if (fileDiff.status === "error") {
    return (
      <ErrorBanner
        title="This file could not load."
        message="The diff did not arrive."
        onRetry={fileDiff.retry}
      />
    );
  }
  const diff = fileDiff.diff;
  if (diff.binary || diff.too_large) {
    return (
      <HashOnly
        note={diff.binary ? binaryNote(diff.change) : tooLargeNote(diff.change)}
        oldHash={diff.old_hash}
        newHash={diff.new_hash}
      />
    );
  }
  const layout = layoutFile(diff.hunks, expanded, limit, {
    keep: threadEnds(rows.threads ?? [], file.path),
  });
  if (layout.rows.length === 0) {
    return (
      <p className="px-14 py-12 text-bar text-text-muted">
        The file has no lines.
      </p>
    );
  }
  return (
    <>
      <DiffRows
        {...rows}
        path={file.path}
        rows={layout.rows}
        onExpand={(key) => {
          setExpanded((old) => new Set([...old, key]));
        }}
      />
      {layout.remaining > 0 ? (
        <div className="border-t border-border-subtle px-14 py-8">
          <Button
            variant="ghost"
            size="sm"
            icon={<ChevronsDown aria-hidden="true" size={13} />}
            onClick={() => {
              setLimit((old) => old + rowLimit);
            }}
          >
            {moreRowsText(layout.remaining)}
          </Button>
        </div>
      ) : null}
    </>
  );
}

export function DiffViewer({
  comparison,
  selectedPath,
  onSelectFile,
  filesTitle,
  ...body
}: DiffViewerProps) {
  const files = listedFiles(comparison.files);
  const shown = shownEntry(files, comparison.warnings, selectedPath);
  if (!shown) return null;
  const list = (
    <FileList
      files={files}
      warnings={comparison.warnings}
      selectedPath={
        shown.kind === "file" ? shown.file.path : shown.warning.path
      }
      onSelectFile={onSelectFile}
      title={filesTitle}
    />
  );
  if (shown.kind === "warning") {
    return (
      <div className="flex min-w-0 flex-1 items-start gap-20">
        {list}
        <section
          aria-label={shown.warning.path}
          className="min-w-0 flex-1 overflow-hidden rounded-l border border-border-subtle bg-surface"
        >
          <header className="flex items-center gap-10 border-b border-border-subtle bg-bg px-14 py-9">
            <h2 className="min-w-0 flex-1 truncate font-mono text-code font-semibold text-text">
              {shown.warning.path}
            </h2>
          </header>
          <p className="flex items-start gap-8 px-14 py-12 text-bar text-text-muted">
            <TriangleAlert
              aria-hidden="true"
              size={14}
              className="mt-2 shrink-0 text-secondary-text"
            />
            {warningNote(shown.warning)}
          </p>
        </section>
      </div>
    );
  }
  const file = shown.file;
  const counts = lineCounts(file);
  const note = changeNote(file);
  return (
    <div className="flex min-w-0 flex-1 items-start gap-20">
      {list}
      <section
        aria-label={file.path}
        className="min-w-0 flex-1 overflow-hidden rounded-l border border-border-subtle bg-surface"
      >
        {/* The rows' fills would cover an inset ring, so the panel has a real border; the head's padding gives up a pixel to each border, so the borders do not change the head's height. */}
        <header className="flex items-center gap-10 border-b border-border-subtle bg-bg px-14 py-9">
          <h2 className="min-w-0 flex-1 truncate font-mono text-code font-semibold text-text">
            {file.path}
          </h2>
          {counts.added ? (
            <span className="font-mono text-meta text-primary-text">
              {counts.added}
            </span>
          ) : null}
          {counts.removed ? (
            <span className="font-mono text-meta text-danger">
              {counts.removed}
            </span>
          ) : null}
          {note ? (
            <span className="text-meta text-text-muted">{note}</span>
          ) : null}
        </header>
        <FileBody key={file.path} file={file} {...body} />
      </section>
    </div>
  );
}
