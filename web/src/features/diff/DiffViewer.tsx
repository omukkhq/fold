import {
  ChevronsDown,
  FoldVertical,
  TriangleAlert,
  UnfoldVertical,
} from "lucide-react";
import { useState, type ReactNode } from "react";
import type { ComparedFile, FileDiff, FileWarning } from "@/api/diff";
import { Button } from "@/components/button/Button";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { DiffRows } from "./DiffRows";
import { FileList } from "./FileList";
import {
  diffSides,
  expandPart,
  gapText,
  hasGaps,
  layoutWithGaps,
  spansOn,
  type FileTextView,
  type GapText,
  type KeptRange,
  type ShownSpans,
  type Span,
} from "./gaps";
import {
  ROW_LIMIT,
  fileBody,
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
  wholeFileText,
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
  /** The new side of the shown file, from which the unchanged lines around its hunks are shown. Without it the file shows its hunks only. */
  fileText?: FileTextView;
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
  /** Drawn in place of the flat file list, such as the proposal's folder tree; the list and the diff then stack on a narrow screen. */
  fileNav?: ReactNode;
  /** Drawn at the end of the file's head, such as a viewed mark. */
  headExtra?: (path: string) => ReactNode;
}

type PanelProps = Omit<
  DiffViewerProps,
  "comparison" | "selectedPath" | "onSelectFile" | "filesTitle" | "fileNav"
> & { file: ComparedFile; split: boolean };

type BodyProps = Omit<PanelProps, "fileText" | "headExtra" | "split"> & {
  gaps: GapText | null;
  spans: readonly Span[];
  whole: boolean;
  onSpans: (spans: Span[]) => void;
  onGapRetry?: () => void;
};

/** The head's path: with a nav beside it the folder part is dimmed before the name, as the tree already names the folders. */
function HeadPath({ path, split }: { path: string; split: boolean }) {
  if (!split) {
    return (
      <h2 className="min-w-0 flex-1 truncate font-mono text-code font-semibold text-text">
        {path}
      </h2>
    );
  }
  const cut = path.lastIndexOf("/") + 1;
  return (
    <h2 className="min-w-0 flex-1 font-mono text-code break-all text-text">
      <span className="font-regular text-text-faint">{path.slice(0, cut)}</span>
      <span className="font-semibold">{path.slice(cut)}</span>
    </h2>
  );
}

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

/** The lines a thread, a mark or the selection stands on: they stay on screen when the gap around them is closed. */
function keptRanges(
  path: string,
  ranges: readonly DiffThread[],
  selection: DiffSelection | null,
): KeptRange[] {
  const own = ranges
    .map((r) => r.range)
    .filter((r) => r.path === path)
    .map((r) => ({ side: r.side, start: r.start_line, end: r.end_line }));
  if (selection?.path === path) {
    own.push({
      side: selection.side,
      start: selection.start_line,
      end: selection.end_line,
    });
  }
  return own;
}

function FileBody({
  file,
  fileDiff,
  rowLimit = ROW_LIMIT,
  gaps,
  spans,
  whole,
  onSpans,
  onGapRetry,
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
  const threads = rows.threads ?? [];
  const kept = keptRanges(
    file.path,
    [...threads, ...(rows.marks ?? [])],
    rows.selection,
  );
  const layout = layoutWithGaps(
    diff.hunks,
    expanded,
    gaps === null ? null : { text: gaps, spans, whole, kept },
    limit,
    { keep: threadEnds(threads, file.path) },
  );
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
        onGap={(row, how) => {
          const result = expandPart(spans, row, how);
          onSpans(result.spans);
          return result.focus;
        }}
        onGapRetry={onGapRetry}
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

/** What the file can show beyond its hunks: null when it has no text source or its text cannot give lines. */
function shownGaps(
  file: ComparedFile,
  fileDiff: FileDiffView,
  fileText: FileTextView | undefined,
): GapText | null {
  if (fileText === undefined || fileBody(file) !== "text") return null;
  if (fileDiff.status !== "ready") return null;
  const diff = fileDiff.diff;
  if (diff.binary || diff.too_large || diff.hunks.length === 0) return null;
  return gapText(diff.hunks, diff.new_hash, fileText);
}

/** One file: its head and its lines. The lines the reader showed belong to the file and the sides it compares, so another file or another version of it starts with its hunks only. */
function FilePanel({ file, split, headExtra, fileText, ...body }: PanelProps) {
  const sides = diffSides(
    body.fileDiff.status === "ready" ? body.fileDiff.diff : null,
  );
  const [shown, setShown] = useState<ShownSpans>({ sides, spans: [] });
  const spans = spansOn(shown, sides);
  const setSpans = (next: readonly Span[]) => {
    setShown({ sides, spans: next });
  };
  const [whole, setWhole] = useState(false);
  const counts = lineCounts(file);
  const note = changeNote(file);
  const gaps = shownGaps(file, body.fileDiff, fileText);
  const hunks =
    body.fileDiff.status === "ready" ? body.fileDiff.diff.hunks : [];
  const toggle =
    gaps !== null &&
    gaps.status !== "error" &&
    (gaps.status === "pending" || hasGaps(hunks, gaps.lines.length));
  return (
    <section
      aria-label={file.path}
      className="min-w-0 flex-1 overflow-hidden rounded-l border border-border-subtle bg-surface"
    >
      {/* The rows' fills would cover an inset ring, so the panel has a real border; the head's padding gives up a pixel to each border, so the borders do not change the head's height. */}
      <header className="flex flex-wrap items-center gap-x-10 gap-y-4 border-b border-border-subtle bg-bg px-14 py-9">
        <HeadPath path={file.path} split={split} />
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
        {toggle ? (
          <Button
            variant="ghost"
            size="sm"
            icon={
              whole ? (
                <FoldVertical aria-hidden="true" size={13} />
              ) : (
                <UnfoldVertical aria-hidden="true" size={13} />
              )
            }
            pending={gaps.status === "pending"}
            pendingLabel="Loading the file…"
            onClick={() => {
              setSpans([]);
              setWhole((old) => !old);
            }}
          >
            {/* On a phone the icon alone keeps the path readable; the label stays the button's name. */}
            <span className="max-sm:sr-only">{wholeFileText(whole)}</span>
          </Button>
        ) : null}
        {headExtra?.(file.path)}
      </header>
      <FileBody
        file={file}
        {...body}
        gaps={gaps}
        spans={spans}
        whole={whole}
        onSpans={setSpans}
        onGapRetry={fileText?.status === "error" ? fileText.retry : undefined}
      />
    </section>
  );
}

export function DiffViewer({
  comparison,
  selectedPath,
  onSelectFile,
  filesTitle,
  fileNav,
  headExtra,
  ...body
}: DiffViewerProps) {
  const files = listedFiles(comparison.files);
  const shown = shownEntry(files, comparison.warnings, selectedPath);
  if (!shown) return null;
  const split = fileNav !== undefined;
  const frame = split
    ? "flex min-w-0 flex-1 flex-col gap-16 lg:flex-row lg:items-start lg:gap-20"
    : "flex min-w-0 flex-1 items-start gap-20";
  const list = fileNav ?? (
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
      <div className={frame}>
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
  return (
    <div className={frame}>
      {list}
      <FilePanel
        key={shown.file.path}
        file={shown.file}
        split={split}
        headExtra={headExtra}
        {...body}
      />
    </div>
  );
}
