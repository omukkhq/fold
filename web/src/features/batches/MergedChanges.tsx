import { useQuery } from "@tanstack/react-query";
import { Diff, GitMerge } from "lucide-react";
import { reviewBatchDiffQuery, reviewBatchFileDiffQuery } from "@/api/queries";
import type { ReviewBatch, ReviewBatchDiff } from "@/api/reviews";
import { HrefLink } from "@/app/HrefLink";
import {
  buttonClasses,
  buttonIconClass,
} from "@/components/button/buttonStyles";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { useFileDiff } from "@/features/diff/fileDiff";
import { currentFile, fileBody, listedFiles } from "@/features/diff/layout";
import { ChangedFiles } from "@/features/proposals/ChangedFiles";
import { useChangedFiles } from "@/features/proposals/changedFilesState";
import { detailHref } from "@/features/proposals/view";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  conflictNote,
  integratesLine,
  mergedEmpty,
  mergedLine,
  mergedViewedKey,
} from "./merged";
import { countsText } from "./present";

function MergedBody({
  slug,
  batch,
  diff,
  path,
  onPath,
}: {
  slug: string;
  batch: ReviewBatch;
  diff: ReviewBatchDiff;
  path: string | undefined;
  onPath: (path: string) => void;
}) {
  const id = batch.id;
  const tree = useChangedFiles(mergedViewedKey(slug, id, diff));
  const files = listedFiles(diff.files);
  const file = currentFile(files, path);
  const fileDiff = useFileDiff(
    reviewBatchFileDiffQuery(slug, id, file?.path ?? ""),
    file !== null && fileBody(file) === "text",
  );
  const line = mergedLine(diff);
  const integrates = integratesLine(batch, diff);
  const conflict = conflictNote(diff);
  const empty = mergedEmpty(diff);
  return (
    <div className="flex min-w-0 flex-col gap-16">
      {line === null ? null : (
        <p className="flex flex-wrap items-center gap-x-8 gap-y-4 text-bar text-text-muted">
          <GitMerge aria-hidden="true" size={14} className="shrink-0" />
          <span className="font-mono text-code text-text">{line}</span>
          {diff.files.length === 0 ? null : (
            <span className="font-mono text-code">
              {countsText(diff.totals)}
            </span>
          )}
          {integrates === null ? null : (
            <span className="text-bar text-text-muted">· {integrates}</span>
          )}
        </p>
      )}
      {conflict === null || diff.conflict === null ? null : (
        <div
          role="alert"
          className="flex flex-wrap items-start gap-12 rounded-m bg-danger-soft px-16 py-12"
        >
          <GitMerge
            aria-hidden="true"
            size={16}
            className="mt-2 shrink-0 text-danger"
          />
          <div className="flex min-w-0 flex-1 flex-col gap-4">
            <p className="text-body font-semibold text-danger">
              {conflict.title}
            </p>
            <p className="text-bar break-words text-text-muted">
              {conflict.text}
            </p>
          </div>
          <HrefLink
            href={detailHref(slug, diff.conflict.proposal, {})}
            className={cn(buttonClasses("secondary", "md"), focusRing)}
          >
            <span className={buttonIconClass("secondary")}>
              <Diff aria-hidden="true" size={15} />
            </span>
            Open {diff.conflict.proposal}
          </HrefLink>
        </div>
      )}
      {empty !== null ? (
        <EmptyPanel icon={GitMerge} title={empty.title}>
          {empty.text}
        </EmptyPanel>
      ) : (
        <ChangedFiles
          state={tree}
          comparison={{ files: diff.files, warnings: [] }}
          selectedPath={file?.path}
          onSelectFile={onPath}
          fileDiff={fileDiff}
          selection={null}
          onSelect={() => undefined}
        />
      )}
    </div>
  );
}

/** The batch's merged change: one diff of main against the tree with every member merged in order, which the daemon builds when the view opens. */
export function MergedChanges({
  slug,
  batch,
  path,
  onPath,
}: {
  slug: string;
  /** The batch as its page read it; a submitted one marks which members integrate. */
  batch: ReviewBatch;
  path: string | undefined;
  onPath: (path: string) => void;
}) {
  const diff = useQuery(reviewBatchDiffQuery(slug, batch.id));
  if (diff.isPending) return <Skeleton />;
  if (diff.isError) {
    return (
      <ErrorBanner
        title="The merged change could not load."
        message="The daemon did not send the merged change of the batch."
        onRetry={() => void diff.refetch()}
      />
    );
  }
  const key = mergedViewedKey(slug, batch.id, diff.data);
  return (
    <MergedBody
      key={key}
      slug={slug}
      batch={batch}
      diff={diff.data}
      path={path}
      onPath={onPath}
    />
  );
}
