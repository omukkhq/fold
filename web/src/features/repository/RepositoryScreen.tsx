import { useQuery } from "@tanstack/react-query";
import { FolderClock, FolderLock, SearchX } from "lucide-react";
import { useState } from "react";
import { useNow } from "@/live/useNow";
import { olderNotListed } from "@/api/pages";
import {
  repositoryQuery,
  revisionQuery,
  revisionsQuery,
  treeQuery,
} from "@/api/repositoryQueries";
import { Mono } from "@/components/mono/Mono";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { CompareBar } from "./CompareBar";
import { FileView } from "./FileView";
import { folderWaits, mainCopyText } from "./describe";
import { isMissingRevision, pickRevision } from "./revision";
import { folderAt, parentDirs, parentOf } from "./search";
import { CommitId } from "@/components/mono/CommitId";
import { GitStatus } from "../git/GitStatus";
import { RevisionList } from "./RevisionList";
import { TreeView } from "./TreeView";

interface RepositoryScreenProps {
  slug: string;
  /** The revision to show; the newest shows when it is absent or the project has no such revision. */
  revision?: string;
  /** A file to open first, with the directories above it expanded. */
  path?: string;
  /** Called with the revision the human picks; the route puts it in the address, which brings it back as `revision`. */
  onSelectRevision: (id: string) => void;
}

export function RepositoryScreen({
  slug,
  revision,
  path,
  onSelectRevision,
}: RepositoryScreenProps) {
  const revisions = useQuery(revisionsQuery(slug));
  const repository = useQuery(repositoryQuery(slug));
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(
    () => new Set(path === undefined ? [] : parentDirs(path)),
  );
  const [openPath, setOpenPath] = useState<string | null>(path ?? null);
  // The clock is read once per mount: the row times are a snapshot, not a ticking display.
  const now = useNow(true, 30_000);

  const listed = revisions.data?.items;
  const pick = listed ? pickRevision(listed, revision) : null;
  // A revision older than the listing reaches is read on its own.
  const unlisted = useQuery({
    ...revisionQuery(slug, pick?.kind === "unlisted" ? pick.id : ""),
    enabled: pick?.kind === "unlisted",
  });
  const shownRevision =
    pick?.kind === "unlisted"
      ? unlisted.data?.id
      : pick?.kind === "listed"
        ? pick.id
        : listed?.[0]?.id;
  // One lookup of the entry's parent decides whether a mentioned path is a folder or a file; until it answers, the path opens nothing.
  const lookup = useQuery({
    ...treeQuery(slug, shownRevision ?? "", parentOf(path ?? "")),
    enabled: path !== undefined && shownRevision !== undefined,
  });
  const folder =
    path !== undefined && lookup.data ? folderAt(lookup.data, path) : null;
  const deciding = path !== undefined && openPath === path && lookup.isPending;
  if (folder && path !== undefined && openPath === path) {
    // Adjusting state while rendering: the folder opens in the tree instead of as a file, and the file view never asks for it.
    setOpenPath(null);
    setExpanded(new Set([...expanded, path]));
  }

  if (revisions.isPending) {
    return (
      <div className="px-24 pt-20 pb-24">
        <Skeleton />
      </div>
    );
  }
  if (revisions.isError) {
    return (
      <ErrorBanner
        title="The repository could not load."
        message="The revisions did not arrive."
        onRetry={() => void revisions.refetch()}
      />
    );
  }
  const { items: history, more } = revisions.data;
  const [newest] = history;
  if (!newest) {
    return (
      <div className="px-24 pt-20 pb-24">
        <EmptyPanel title="No revisions yet">
          The project has no main history. Integrate a proposal to create r1.
        </EmptyPanel>
      </div>
    );
  }
  if (pick?.kind === "unlisted" && !unlisted.data) {
    if (unlisted.isError && isMissingRevision(unlisted.error)) {
      return (
        <div className="px-24 pt-20 pb-24">
          <EmptyPanel icon={SearchX} title={`${pick.id} does not exist`}>
            The project has no such revision.
          </EmptyPanel>
        </div>
      );
    }
    if (unlisted.isError) {
      return (
        <ErrorBanner
          title="The revision could not load."
          message="The daemon did not answer."
          onRetry={() => void unlisted.refetch()}
        />
      );
    }
    return (
      <div className="px-24 pt-20 pb-24">
        <Skeleton />
      </div>
    );
  }
  const selected =
    pick?.kind === "unlisted" ? pick.id : (shownRevision ?? newest.id);
  const selectedCommit = (
    unlisted.data ?? history.find((r) => r.id === selected)
  )?.commit;
  const onlyFirst = history.length === 1;
  const mainCopy = mainCopyText(repository.data);

  function toggle(path: string) {
    const next = new Set(expanded);
    if (!next.delete(path)) {
      next.add(path);
    }
    setExpanded(next);
  }

  return (
    <div className="flex flex-1 flex-col gap-20 px-24 pt-20 pb-24 lg:flex-row lg:items-start">
      <section
        aria-label="History"
        className="flex flex-col gap-12 lg:w-history lg:shrink-0"
      >
        <div className="flex flex-col gap-4">
          <h1 className="text-screen leading-snug font-semibold text-text">
            Repository
          </h1>
          <p className="text-body text-text-muted">
            Accepted history only. Each revision is one integration.
          </p>
        </div>
        <RevisionList
          revisions={history}
          selected={selected}
          now={now}
          onSelect={onSelectRevision}
        />
        {more ? (
          <p className="text-bar text-text-muted">
            {olderNotListed(history.length, "revisions")}
          </p>
        ) : null}
        {onlyFirst ? (
          <p className="text-bar text-text-muted">
            Only r1 exists, so there is nothing to compare yet.
          </p>
        ) : null}
      </section>
      <section
        aria-label="Tree"
        className="flex min-w-0 flex-1 flex-col gap-12"
      >
        <div className="flex min-h-32 flex-wrap items-center justify-between gap-10">
          <span className="flex items-center gap-10">
            <Mono className="text-bar font-semibold text-text">{selected}</Mono>
            <CommitId commit={selectedCommit} />
          </span>
          {onlyFirst ? null : (
            <CompareBar
              key={selected}
              slug={slug}
              revisions={history}
              selected={selected}
            />
          )}
        </div>
        <TreeView
          slug={slug}
          revision={selected}
          expanded={expanded}
          openPath={openPath}
          onToggle={toggle}
          onOpenFile={setOpenPath}
        />
        {openPath === null || deciding ? null : (
          <FileView slug={slug} revision={selected} path={openPath} />
        )}
        {mainCopy === null ? null : (
          <p className="flex items-center gap-8 rounded-m bg-sunken px-12 py-10 text-bar text-text-muted">
            {folderWaits(repository.data) ? (
              <FolderClock aria-hidden size={15} className="shrink-0" />
            ) : (
              <FolderLock aria-hidden size={15} className="shrink-0" />
            )}
            {mainCopy}
          </p>
        )}
        <GitStatus slug={slug} variant="panel" />
      </section>
    </div>
  );
}
