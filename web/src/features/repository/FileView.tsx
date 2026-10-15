import { useQuery } from "@tanstack/react-query";
import { FileX, Link2, Ruler } from "lucide-react";
import { Mono } from "@/components/mono/Mono";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { fileState, formatSize } from "./describe";
import { fileQuery } from "@/api/repositoryQueries";

interface FileViewProps {
  slug: string;
  revision: string;
  path: string;
}

/** A read-only view of one file. A binary or too-large file shows its size and hash and no content; a symlink shows its target text and is never followed. */
export function FileView({ slug, revision, path }: FileViewProps) {
  const file = useQuery(fileQuery(slug, revision, path));
  const shown = file.data ? fileState(file.data) : null;
  return (
    <section
      aria-label={`File ${path}`}
      className="flex flex-col overflow-hidden rounded-l bg-surface inset-ring inset-ring-border-subtle"
    >
      <header className="flex items-center gap-10 border-b border-border-subtle px-14 py-10">
        <Mono className="text-bar font-semibold text-text">{path}</Mono>
        <Mono className="text-label text-text-muted">{revision}</Mono>
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
        <pre className="overflow-x-auto px-14 py-12 font-mono text-code leading-normal text-text">
          {shown.text}
        </pre>
      ) : null}
      {shown?.kind === "symlink" ? (
        <p className="flex items-center gap-8 px-14 py-12 text-bar text-text-muted">
          <Link2 aria-hidden size={15} />
          <Mono className="text-text">{`→ ${shown.target}`}</Mono>
        </p>
      ) : null}
      {file.data &&
      (shown?.kind === "binary" || shown?.kind === "too_large") ? (
        <div className="flex flex-col gap-6 px-14 py-12">
          <p className="flex items-center gap-8 text-bar text-text-muted">
            {shown.kind === "binary" ? (
              <FileX aria-hidden size={15} />
            ) : (
              <Ruler aria-hidden size={15} />
            )}
            {shown.kind === "binary"
              ? "Binary file, no content shown."
              : "File too large, no content shown."}
          </p>
          <Mono className="text-label text-text-muted">
            {`${formatSize(file.data.size)} · ${file.data.hash}`}
          </Mono>
        </div>
      ) : null}
    </section>
  );
}
