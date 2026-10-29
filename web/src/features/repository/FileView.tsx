import { Id } from "@/components/id/Id";
import { useQuery } from "@tanstack/react-query";
import { FileX, Link2, Ruler } from "lucide-react";
import { useState } from "react";
import { Mono } from "@/components/mono/Mono";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { requestsQuery } from "@/api/queries";
import { DiffRows } from "@/features/diff/DiffRows";
import type { DiffSelection } from "@/features/diff/selection";
import { revisionRequestMarks } from "@/features/requests/codeLocation";
import { CodeRequestComposer } from "@/features/requests/CodeRequestComposer";
import { RequestMarker } from "@/features/requests/RequestMarker";
import { fileState, formatSize } from "./describe";
import { fileRows } from "./fileLines";
import { fileQuery } from "@/api/repositoryQueries";

interface FileViewProps {
  slug: string;
  revision: string;
  path: string;
}

/** A read-only view of one file, highlighted when Shiki has a grammar for its name. Selected lines take a request, and the requests made on lines of this revision mark them. A binary or too-large file shows its size and hash and no content; a symlink shows its target text and is never followed. */
export function FileView({ slug, revision, path }: FileViewProps) {
  const file = useQuery(fileQuery(slug, revision, path));
  const requests = useQuery(requestsQuery(slug));
  const [selection, setSelection] = useState<DiffSelection | null>(null);
  const shown = file.data ? fileState(file.data) : null;
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
            rows={fileRows(shown.text)}
            selection={selection}
            onSelect={setSelection}
            onExpand={() => undefined}
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
            renderComposer={(sel) => (
              <CodeRequestComposer
                key={`${String(sel.start_line)}:${String(sel.end_line)}`}
                slug={slug}
                anchor={{ kind: "revision", revision }}
                selection={sel}
                onDone={() => {
                  setSelection(null);
                }}
              />
            )}
          />
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
