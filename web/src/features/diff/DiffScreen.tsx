import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import type { Comparison } from "@/api/diff";
import { fileDiffQuery } from "@/api/queries";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { DiffHeader } from "./DiffHeader";
import { DiffViewer, type FileDiffView } from "./DiffViewer";
import { currentFile, fileBody, listedFiles } from "./layout";
import { isWorkingChanges, workingNote } from "./present";
import type { DiffSearch } from "./search";
import type { DiffSelection } from "./selection";

interface DiffScreenProps {
  slug: string;
  search: DiffSearch;
  comparison: Comparison | null;
  onSelectFile: (path: string) => void;
}

function useFileDiff(
  slug: string,
  search: DiffSearch,
  path: string | null,
  enabled: boolean,
): FileDiffView {
  const query = useQuery({
    ...fileDiffQuery(slug, search.from, search.to, path ?? ""),
    enabled: enabled && path !== null,
  });
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

export function DiffScreen({
  slug,
  search,
  comparison,
  onSelectFile,
}: DiffScreenProps) {
  const [selection, setSelection] = useState<DiffSelection | null>(null);
  const [now] = useState(() => Date.now());
  const files = comparison ? listedFiles(comparison.files) : [];
  const file = currentFile(files, search.path);
  const fileDiff = useFileDiff(
    slug,
    search,
    file?.path ?? null,
    file !== null && fileBody(file) === "text",
  );

  if (comparison === null) {
    return (
      <div className="p-24">
        <EmptyPanel title="Nothing to compare">
          {search.from === undefined
            ? `fold has no comparison for ${search.to}.`
            : `fold has no comparison of ${search.from} with ${search.to}.`}
        </EmptyPanel>
      </div>
    );
  }
  return (
    <div className="flex flex-col">
      <DiffHeader slug={slug} comparison={comparison} files={files} now={now} />
      <div className="flex items-start gap-20 px-24 pt-4 pb-24">
        {files.length === 0 ? (
          <EmptyPanel title="No changes" className="flex-1">
            {`${comparison.from.id} and ${comparison.to.id} hold the same files.`}
          </EmptyPanel>
        ) : (
          <DiffViewer
            comparison={comparison}
            selectedPath={file?.path}
            onSelectFile={onSelectFile}
            fileDiff={fileDiff}
            selection={selection}
            onSelect={setSelection}
          />
        )}
        {isWorkingChanges(comparison) ? (
          <aside className="flex w-side-panel shrink-0 flex-col gap-8">
            <h2 className="text-overline leading-snug font-semibold text-primary-text uppercase">
              This view
            </h2>
            <p className="text-bar leading-snug text-text-muted">
              {workingNote(comparison)}
            </p>
          </aside>
        ) : null}
      </div>
    </div>
  );
}
