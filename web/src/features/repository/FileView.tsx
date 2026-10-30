import { Id } from "@/components/id/Id";
import { useQuery } from "@tanstack/react-query";
import { ChevronsDown, FileX, Link2, Ruler } from "lucide-react";
import { useMemo, useState } from "react";
import { Mono } from "@/components/mono/Mono";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { requestsQuery } from "@/api/queries";
import { Button } from "@/components/button/Button";
import { DiffRows } from "@/features/diff/DiffRows";
import { ROW_LIMIT } from "@/features/diff/layout";
import { moreRowsText } from "@/features/diff/present";
import type { DiffSelection } from "@/features/diff/selection";
import { revisionRequestMarks } from "@/features/requests/codeLocation";
import { CodeRequestComposer } from "@/features/requests/CodeRequestComposer";
import { RequestMarker } from "@/features/requests/RequestMarker";
import { fileState, formatSize } from "./describe";
import { fileRows, firstRowLimit, openedSelection } from "./fileLines";
import { fileQuery } from "@/api/repositoryQueries";

interface FileViewProps {
  slug: string;
  revision: string;
  path: string;
  /** The line a link opened the file at: the view shows enough rows to hold it, selects it and puts focus on it. */
  line?: number;
}

/** A read-only view of one file, highlighted when Shiki has a grammar for its name. Selected lines take a request, and the requests made on lines of this revision mark them. A binary or too-large file shows its size and hash and no content; a symlink shows its target text and is never followed. The caller keys it by revision and path, so its selection and row count start again with each file. */
export function FileView({ slug, revision, path, line }: FileViewProps) {
  const file = useQuery(fileQuery(slug, revision, path));
  const requests = useQuery(requestsQuery(slug));
  // undefined until the reader selects: until then the line the link opened the file at stays selected.
  const [chosen, setChosen] = useState<DiffSelection | null | undefined>(
    undefined,
  );
  const [limit, setLimit] = useState(() => firstRowLimit(line));
  const shown = file.data ? fileState(file.data) : null;
  const text = shown?.kind === "text" ? shown.text : null;
  const rows = useMemo(() => (text === null ? [] : fileRows(text)), [text]);
  const opened = openedSelection(path, line, rows);
  const selection = chosen === undefined ? opened : chosen;
  const byId = new Map((requests.data ?? []).map((r) => [r.id, r]));
  return (
    <section
      aria-label={`File ${path}`}
      className="flex flex-col overflow-hidden rounded-m bg-surface"
    >
      <header className="flex items-center gap-10 px-16 pt-12 pb-6">
        <Mono className="min-w-0 truncate text-14 font-semibold text-text">
          {path}
        </Mono>
        <Id className="ml-auto text-13 text-text-2">{revision}</Id>
      </header>
      {file.isPending ? <Skeleton className="m-12" /> : null}
      {file.isError ? (
        <ErrorBanner
          title="The file could not load."
          message="The content did not arrive."
          onRetry={() => void file.refetch()}
        />
      ) : null}
      {shown?.kind === "text" ? (
        <div className="pt-6 pb-14">
          <DiffRows
            variant="file"
            path={path}
            rows={rows.slice(0, limit)}
            selection={selection}
            onSelect={setChosen}
            onExpand={() => undefined}
            focusOn={
              opened === null ? null : { side: "new", line: opened.start_line }
            }
            marks={revisionRequestMarks(requests.data ?? [], revision, "new")}
            renderMark={(mark, first) => {
              const request = byId.get(mark.id);
              return request ? (
                <RequestMarker
                  slug={slug}
                  request={request}
                  focusable={first}
                />
              ) : null;
            }}
            renderComposer={(sel, focusLine) => (
              <CodeRequestComposer
                key={`${String(sel.start_line)}:${String(sel.end_line)}`}
                slug={slug}
                anchor={{ kind: "revision", revision }}
                selection={sel}
                focusLine={focusLine}
                onDone={() => {
                  setChosen(null);
                }}
              />
            )}
          />
          {rows.length > limit ? (
            <div className="bg-diff-hunk px-8 py-3">
              <Button
                variant="ghost"
                size="sm"
                className="text-accent hover:bg-surface hover:text-accent"
                icon={<ChevronsDown aria-hidden="true" size={14} />}
                onClick={() => {
                  setLimit((old) => old + ROW_LIMIT);
                }}
              >
                {moreRowsText(rows.length - limit)}
              </Button>
            </div>
          ) : null}
        </div>
      ) : null}
      {shown?.kind === "symlink" ? (
        <p className="flex items-center gap-8 px-16 pt-6 pb-14 text-14 text-text-2">
          <Link2 aria-hidden size={16} />
          <Mono className="text-text">{`→ ${shown.target}`}</Mono>
        </p>
      ) : null}
      {file.data &&
      (shown?.kind === "binary" || shown?.kind === "too_large") ? (
        <div className="flex flex-col gap-6 px-16 pt-6 pb-14">
          <p className="flex items-center gap-8 text-14 text-text-2">
            {shown.kind === "binary" ? (
              <FileX aria-hidden size={16} />
            ) : (
              <Ruler aria-hidden size={16} />
            )}
            {shown.kind === "binary"
              ? "Binary file, no content shown."
              : "File too large, no content shown."}
          </p>
          <Mono className="text-13 text-text-2">
            {`${formatSize(file.data.size)} · ${file.data.hash}`}
          </Mono>
        </div>
      ) : null}
    </section>
  );
}
