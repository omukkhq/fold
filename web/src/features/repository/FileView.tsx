import { useQuery } from "@tanstack/react-query";
import { FileX } from "lucide-react";
import { Mono } from "@/components/mono/Mono";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { formatSize } from "./describe";
import { fileQuery } from "./queries";

interface FileViewProps {
  slug: string;
  revision: string;
  path: string;
}

/** A read-only view of one file. A binary file shows its size and hash and no content. */
export function FileView({ slug, revision, path }: FileViewProps) {
  const file = useQuery(fileQuery(slug, revision, path));
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
      {file.data?.kind === "text" ? (
        <pre className="overflow-x-auto px-14 py-12 font-mono text-code leading-normal text-text">
          {file.data.content}
        </pre>
      ) : null}
      {file.data?.kind === "binary" ? (
        <div className="flex flex-col gap-6 px-14 py-12">
          <p className="flex items-center gap-8 text-bar text-text-muted">
            <FileX aria-hidden size={15} />
            Binary file, no content shown.
          </p>
          <Mono className="text-label text-text-muted">
            {`${formatSize(file.data.size)} · ${file.data.hash}`}
          </Mono>
        </div>
      ) : null}
    </section>
  );
}
