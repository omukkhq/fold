import { useQuery } from "@tanstack/react-query";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useState } from "react";
import type { ComparedFile } from "@/api/diff";
import { folderDiffQuery, folderFileDiffQuery } from "@/api/repositoryQueries";
import { Mono } from "@/components/mono/Mono";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { DiffViewer, type FileDiffView } from "@/features/diff/DiffViewer";
import { focusRing } from "@/lib/focus";
import { cn } from "@/lib/cn";
import {
  flaggedFacts,
  flaggedFiles,
  folderFailure,
  type FlaggedFile,
} from "./flaggedFiles";

function noop() {}

/** One changed file's hunks against the revision the folder holds, drawn by the shared diff viewer without its file list. */
function FileDiffPanel({ slug, file }: { slug: string; file: ComparedFile }) {
  const query = useQuery(folderFileDiffQuery(slug, file.path));
  const fileDiff: FileDiffView = query.isError
    ? {
        status: "error",
        retry: () => {
          void query.refetch();
        },
      }
    : query.data
      ? { status: "ready", diff: query.data }
      : { status: "pending" };
  return (
    <DiffViewer
      comparison={{ files: [file], warnings: [] }}
      selectedPath={file.path}
      onSelectFile={noop}
      fileDiff={fileDiff}
      selection={null}
      onSelect={noop}
      fileNav={false}
    />
  );
}

/** A file's row: its path and change, which opens its diff in place. */
function FileRow({ slug, flagged }: { slug: string; flagged: FlaggedFile }) {
  const [open, setOpen] = useState(false);
  const Chevron = open ? ChevronDown : ChevronRight;
  return (
    <li className="flex min-w-0 flex-col gap-6">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => {
          setOpen((old) => !old);
        }}
        className={cn(
          "group flex min-w-0 items-start gap-6 rounded-s px-4 py-2 text-left",
          focusRing,
        )}
      >
        <Chevron
          aria-hidden="true"
          size={14}
          className="mt-3 shrink-0 text-text-2"
        />
        <span className="flex min-w-0 flex-wrap items-baseline gap-x-8">
          <Mono className="min-w-0 wrap-anywhere text-text underline-offset-2 group-hover:underline">
            {flagged.path}
          </Mono>
          <span className="text-13 text-text-2">{flaggedFacts(flagged)}</span>
        </span>
      </button>
      {open ? (
        flagged.file === null ? (
          <p className="pl-24 text-13 text-text-2">
            {
              "The project folder holds the revision's version of this file again."
            }
          </p>
        ) : (
          <FileDiffPanel slug={slug} file={flagged.file} />
        )
      ) : null}
    </li>
  );
}

/** The named files of the project folder, each of which opens its diff against the revision the folder holds; `more` counts the files a list leaves out. */
export function FolderFiles({
  slug,
  paths,
  more = 0,
}: {
  slug: string;
  paths: readonly string[];
  more?: number;
}) {
  const comparison = useQuery(folderDiffQuery(slug));
  if (comparison.isError) {
    return (
      <ErrorBanner
        className="rounded-m px-12"
        title="The changes could not load."
        message={folderFailure(comparison.error)}
        onRetry={() => void comparison.refetch()}
      />
    );
  }
  if (!comparison.data) {
    return <Skeleton />;
  }
  return (
    <ul className="flex min-w-0 flex-col gap-2">
      {flaggedFiles(comparison.data.files, paths).map((flagged) => (
        <FileRow key={flagged.path} slug={slug} flagged={flagged} />
      ))}
      {more > 0 ? (
        <li className="pl-24 text-13 text-text-2">and {String(more)} more</li>
      ) : null}
    </ul>
  );
}
