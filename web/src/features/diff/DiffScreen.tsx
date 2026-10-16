import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useNow } from "@/live/useNow";
import type { Comparison } from "@/api/diff";
import { fileDiffQuery, workspaceQuery } from "@/api/queries";
import { repositoryQuery } from "@/api/repositoryQueries";
import { HrefLink } from "@/app/HrefLink";
import { EmptyPanel } from "@/components/states/EmptyPanel";
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
    <aside className="flex w-side-panel shrink-0 flex-col gap-20">
      <section className="flex flex-col gap-8">
        <h2 className="text-overline leading-snug font-semibold text-primary-text uppercase">
          This view
        </h2>
        <p className="text-bar leading-snug text-text-muted">
          {workingNote(comparison)}
        </p>
      </section>
      {options.length > 0 ? (
        <nav aria-label="Compare with" className="flex flex-col gap-8">
          <h2 className="text-overline leading-snug font-semibold text-primary-text uppercase">
            Compare with
          </h2>
          <ul className="flex flex-col gap-8">
            {options.map((option) => (
              <li key={option.from}>
                <HrefLink
                  href={option.href}
                  aria-current={option.current ? "page" : undefined}
                  className={cn(
                    "rounded-s text-body text-text hover:text-primary-text",
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
      <DiffHeader
        slug={slug}
        comparison={comparison}
        files={files}
        now={now}
        readAt={comparison.source === "working" ? readAt : null}
      />
      <div className="flex items-start gap-20 px-24 pt-4 pb-24">
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
          />
        )}
        {comparison.to.kind === "workspace" ? (
          <WorkspaceSide slug={slug} comparison={comparison} />
        ) : null}
      </div>
    </div>
  );
}
