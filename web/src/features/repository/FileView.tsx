import { IdText } from "@/components/id/IdText";
import { useQuery } from "@tanstack/react-query";
import { FileX, Link2, Ruler } from "lucide-react";
import { SyntaxText } from "@/components/code/SyntaxText";
import { Mono } from "@/components/mono/Mono";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { CODE_BOX } from "@/components/text/textFlow";
import { cn } from "@/lib/cn";
import { fileState, formatSize } from "./describe";
import { fileQuery } from "@/api/repositoryQueries";
import { languageForPath } from "@/syntax/languages";

interface FileViewProps {
  slug: string;
  revision: string;
  path: string;
}

/** A read-only view of one file, highlighted when Shiki has a grammar for its name. A binary or too-large file shows its size and hash and no content; a symlink shows its target text and is never followed. */
export function FileView({ slug, revision, path }: FileViewProps) {
  const file = useQuery(fileQuery(slug, revision, path));
  const shown = file.data ? fileState(file.data) : null;
  return (
    <section
      aria-label={`File ${path}`}
      className="flex flex-col overflow-hidden rounded-l bg-surface inset-ring inset-ring-line"
    >
      <header className="flex items-center gap-10 border-b border-line px-14 py-10">
        <Mono className="text-14 font-semibold text-text">{path}</Mono>
        <IdText className="text-13 text-text-2">{revision}</IdText>
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
        <pre
          className={cn(
            CODE_BOX,
            "px-14 py-12 font-mono text-13 leading-normal text-text",
          )}
        >
          <SyntaxText code={shown.text} language={languageForPath(path)} />
        </pre>
      ) : null}
      {shown?.kind === "symlink" ? (
        <p className="flex items-center gap-8 px-14 py-12 text-14 text-text-2">
          <Link2 aria-hidden size={15} />
          <Mono className="text-text">{`→ ${shown.target}`}</Mono>
        </p>
      ) : null}
      {file.data &&
      (shown?.kind === "binary" || shown?.kind === "too_large") ? (
        <div className="flex flex-col gap-6 px-14 py-12">
          <p className="flex items-center gap-8 text-14 text-text-2">
            {shown.kind === "binary" ? (
              <FileX aria-hidden size={15} />
            ) : (
              <Ruler aria-hidden size={15} />
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
