import { ChevronsDown } from "lucide-react";
import { useState, type ReactNode } from "react";
import type { ComparedFile, Comparison, FileDiff } from "@/api/diff";
import { Button } from "@/components/button/Button";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { DiffRows } from "./DiffRows";
import { FileList } from "./FileList";
import {
  ROW_LIMIT,
  currentFile,
  fileBody,
  layoutFile,
  listedFiles,
} from "./layout";
import { binaryNote, changeNote, lineCounts, moreRowsText } from "./present";
import type { DiffSelection, DiffThread } from "./selection";

/** The selected file's hunks as its query holds them. */
export type FileDiffView =
  | { status: "pending" }
  | { status: "error"; retry: () => void }
  | { status: "ready"; diff: FileDiff };

export interface DiffViewerProps {
  comparison: Comparison;
  selectedPath: string | undefined;
  onSelectFile: (path: string) => void;
  /** Read only for a file whose body is text; a binary file and an unchanged rename need no hunks. */
  fileDiff: FileDiffView;
  selection: DiffSelection | null;
  onSelect: (selection: DiffSelection | null) => void;
  threads?: readonly DiffThread[];
  renderThread?: (thread: DiffThread) => ReactNode;
  /** Shown under the last selected line. */
  renderComposer?: (selection: DiffSelection) => ReactNode;
  /** The rows a file shows before "Show more". */
  rowLimit?: number;
}

type BodyProps = Omit<
  DiffViewerProps,
  "comparison" | "selectedPath" | "onSelectFile"
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
      <div className="flex flex-col gap-8 px-14 py-12">
        <p className="text-bar text-text-muted">{binaryNote(file.change)}</p>
        <dl className="flex flex-col gap-4">
          <Hash side="old" hash={file.old_hash} />
          <Hash side="new" hash={file.new_hash} />
        </dl>
      </div>
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
  const layout = layoutFile(fileDiff.diff.hunks, expanded, limit);
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
  ...body
}: DiffViewerProps) {
  const files = listedFiles(comparison.files);
  const file = currentFile(files, selectedPath);
  if (!file) return null;
  const counts = lineCounts(file);
  const note = changeNote(file);
  return (
    <div className="flex min-w-0 flex-1 items-start gap-20">
      <FileList
        files={files}
        selectedPath={file.path}
        onSelectFile={onSelectFile}
      />
      <section
        aria-label={file.path}
        className="min-w-0 flex-1 overflow-hidden rounded-l border border-border-subtle bg-surface"
      >
        {/* The rows' fills would cover an inset ring, so the panel has a real border; the head's padding gives a pixel to each border, as the frame's inner strokes take it. */}
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
