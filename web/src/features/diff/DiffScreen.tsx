import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useNow } from "@/live/useNow";
import type { Comparison } from "@/api/diff";
import { fileDiffQuery, requestsQuery, workspaceQuery } from "@/api/queries";
import { repositoryQuery } from "@/api/repositoryQueries";
import { HrefLink } from "@/app/HrefLink";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import {
  compareMarks,
  revisionAnchor,
  sideSelection,
} from "@/features/requests/codeLocation";
import { CodeRequestComposer } from "@/features/requests/CodeRequestComposer";
import { RequestMarker } from "@/features/requests/RequestMarker";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { DiffHeader } from "./DiffHeader";
import { DiffViewer, type FileDiffView } from "./DiffViewer";
import { fileBody, listedFiles, shownEntry } from "./layout";
import { compareOptions, workingNote } from "./present";
import type { DiffSearch } from "./search";
import type { DiffSelection } from "./selection";

interface DiffScreenProps {
  slug: string;
  search: DiffSearch;
  comparison: Comparison | null;
  /** When the comparison was read; the event stream has it read again on each change of a workspace side. */
  readAt: number | null;
  onSelectFile: (path: string) => void;
}

function useFileDiff(
  slug: string,
  search: DiffSearch,
  path: string | null,
): FileDiffView {
  const query = useQuery({
    ...fileDiffQuery(slug, search.from, search.to, path ?? ""),
    enabled: path !== null,
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

/** The side panel of a workspace diff: where its files come from and what else it can be compared with. */
function WorkspaceSide({
  slug,
  comparison,
}: {
  slug: string;
  comparison: Comparison;
}) {
  const workspace = useQuery(workspaceQuery(slug, comparison.to.id));
  const repository = useQuery(repositoryQuery(slug));
  const head = repository.data?.head ?? null;
  const options = workspace.data
    ? compareOptions(slug, comparison, {
        base: workspace.data.base,
        main:
          repository.data && head !== null
            ? { head, recent: repository.data.recent }
            : null,
        checkpoints: workspace.data.live?.checkpoints ?? [],
      })
    : [];
  return (
    <aside className="flex w-full shrink-0 flex-col gap-20 lg:w-side-panel">
      <section className="flex flex-col gap-8">
        <h2 className="font-title text-13 leading-tight font-semibold text-text-2">
          This view
        </h2>
        <p className="text-14 leading-tight text-text-2">
          {workingNote(comparison)}
        </p>
      </section>
      {options.length > 0 ? (
        <nav aria-label="Compare with" className="flex flex-col gap-8">
          <h2 className="font-title text-13 leading-tight font-semibold text-text-2">
            Compare with
          </h2>
          <ul className="flex flex-col gap-8">
            {options.map((option) => (
              <li key={option.from}>
                <HrefLink
                  href={option.href}
                  aria-current={option.current ? "page" : undefined}
                  className={cn(
                    "rounded-s text-15 text-text hover:text-accent",
                    option.current && "font-semibold",
                    focusRing,
                  )}
                >
                  {option.text}
                </HrefLink>
              </li>
            ))}
          </ul>
        </nav>
      ) : null}
    </aside>
  );
}

export function DiffScreen({
  slug,
  search,
  comparison,
  readAt,
  onSelectFile,
}: DiffScreenProps) {
  const [selection, setSelection] = useState<DiffSelection | null>(null);
  const now = useNow(true, 30_000);
  const files = comparison ? listedFiles(comparison.files) : [];
  const shown = shownEntry(files, comparison?.warnings ?? [], search.path);
  const file = shown?.kind === "file" ? shown.file : null;
  const fileDiff = useFileDiff(
    slug,
    search,
    file !== null && fileBody(file) === "text" ? file.path : null,
  );
  const requests = useQuery(requestsQuery(slug));

  if (comparison === null) {
    return (
      <div className="px-16 py-24 md:px-24">
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
      <DiffHeader
        slug={slug}
        comparison={comparison}
        files={files}
        now={now}
        readAt={comparison.source === "working" ? readAt : null}
      />
      <div className="flex flex-col gap-20 px-16 pt-4 pb-24 md:px-24 lg:flex-row lg:items-start">
        {shown === null ? (
          <EmptyPanel title="No changes" className="flex-1">
            {`${comparison.from.id} and ${comparison.to.id} hold the same files.`}
          </EmptyPanel>
        ) : (
          <DiffViewer
            comparison={comparison}
            selectedPath={search.path}
            onSelectFile={onSelectFile}
            fileDiff={fileDiff}
            selection={selection}
            onSelect={setSelection}
            marks={compareMarks(requests.data ?? [], comparison, file)}
            renderMark={(mark, first) => {
              const request = requests.data?.find((r) => r.id === mark.id);
              return request ? (
                <RequestMarker
                  slug={slug}
                  request={request}
                  focusable={first}
                />
              ) : null;
            }}
            renderComposer={(sel, focusLine) => {
              const anchor = revisionAnchor(
                sel.side === "new" ? comparison.to : comparison.from,
              );
              return anchor === null ? null : (
                <CodeRequestComposer
                  key={`${sel.path}:${sel.side}:${String(sel.start_line)}:${String(sel.end_line)}`}
                  slug={slug}
                  anchor={anchor}
                  selection={sideSelection(sel, file)}
                  focusLine={focusLine}
                  onDone={() => {
                    setSelection(null);
                  }}
                />
              );
            }}
          />
        )}
        {comparison.to.kind === "workspace" ? (
          <WorkspaceSide slug={slug} comparison={comparison} />
        ) : null}
      </div>
    </div>
  );
}
