import { useQuery } from "@tanstack/react-query";
import { Diff, GitMerge } from "lucide-react";
import { useState } from "react";
import { reviewBatchDiffQuery, reviewBatchFileDiffQuery } from "@/api/queries";
import type { ReviewBatchDiff } from "@/api/reviews";
import { HrefLink } from "@/app/HrefLink";
import {
  buttonClasses,
  buttonIconClass,
} from "@/components/button/buttonStyles";
import { Checkbox } from "@/components/form/controls";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { DiffViewer, type FileDiffView } from "@/features/diff/DiffViewer";
import { currentFile, fileBody, listedFiles } from "@/features/diff/layout";
import { useViewed } from "@/features/proposals/ChangesTab";
import { FileTree } from "@/features/proposals/FileTree";
import {
  changeTree,
  folderPaths,
  toggleFolder,
  treeFiles,
} from "@/features/proposals/folderTree";
import { detailHref } from "@/features/proposals/view";
import { viewedText } from "@/features/proposals/viewed";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  conflictNote,
  mergedEmpty,
  mergedLine,
  mergedViewedKey,
} from "./merged";
import { countsText } from "./present";

function useFileDiff(slug: string, id: string, path: string | null) {
  const query = useQuery({
    ...reviewBatchFileDiffQuery(slug, id, path ?? ""),
    enabled: path !== null,
  });
  if (query.isError) {
    return {
      status: "error",
      retry: () => {
        void query.refetch();
      },
    } satisfies FileDiffView;
  }
  return query.data
    ? ({ status: "ready", diff: query.data } satisfies FileDiffView)
    : ({ status: "pending" } satisfies FileDiffView);
}

function MergedBody({
  slug,
  id,
  diff,
  path,
  onPath,
}: {
  slug: string;
  id: string;
  diff: ReviewBatchDiff;
  path: string | undefined;
  onPath: (path: string) => void;
}) {
  const [closed, setClosed] = useState<ReadonlySet<string>>(new Set());
  const [filter, setFilter] = useState("");
  const { viewed, toggle } = useViewed(mergedViewedKey(slug, id, diff));
  const tree = changeTree(diff.files);
  const files = listedFiles(diff.files);
  const file = currentFile(files, path);
  const fileDiff = useFileDiff(
    slug,
    id,
    file !== null && fileBody(file) === "text" ? file.path : null,
  );
  const line = mergedLine(diff);
  const conflict = conflictNote(diff);
  const empty = mergedEmpty(diff);
  const paths = treeFiles(tree).map((f) => f.path);
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
        <DiffViewer
          comparison={{ files: diff.files, warnings: [] }}
          selectedPath={file?.path}
          onSelectFile={onPath}
          fileNav={
            <FileTree
              nodes={tree}
              selectedPath={file?.path ?? null}
              viewed={viewed}
              viewedText={viewedText(viewed, paths)}
              closed={closed}
              filter={filter}
              onFilter={setFilter}
              onToggle={(folder) => {
                setClosed((old) => toggleFolder(old, folder));
              }}
              onExpandAll={() => {
                setClosed(new Set());
              }}
              onCollapseAll={() => {
                setClosed(new Set(folderPaths(tree)));
              }}
              onSelect={onPath}
            />
          }
          headExtra={(shown) => (
            <Checkbox
              label="Viewed"
              checked={viewed.has(shown)}
              onCheckedChange={() => {
                toggle(shown);
              }}
            />
          )}
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
  id,
  path,
  onPath,
}: {
  slug: string;
  id: string;
  path: string | undefined;
  onPath: (path: string) => void;
}) {
  const diff = useQuery(reviewBatchDiffQuery(slug, id));
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
  const key = mergedViewedKey(slug, id, diff.data);
  return (
    <MergedBody
      key={key}
      slug={slug}
      id={id}
      diff={diff.data}
      path={path}
      onPath={onPath}
    />
  );
}
