import { useQuery } from "@tanstack/react-query";
import {
  ChevronDown,
  ChevronRight,
  File,
  FileQuestion,
  Folder,
  FolderOpen,
  Link2,
} from "lucide-react";
import type { TreeEntry } from "@/api/repository";
import { Mono } from "@/components/mono/Mono";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { formatSize, isOpenable, symlinkLabel } from "./describe";
import { treeQuery } from "@/api/repositoryQueries";

interface TreeViewProps {
  slug: string;
  revision: string;
  expanded: ReadonlySet<string>;
  openPath: string | null;
  onToggle: (path: string) => void;
  onOpenFile: (path: string) => void;
}

/** The tree at one revision. A directory loads its children when it first opens; a symlink opens like a file and shows its target text, and is never followed. An entry whose name is not UTF-8 shows but cannot be opened. */
export function TreeView(props: TreeViewProps) {
  return (
    <div className="rounded-m bg-surface p-6">
      <TreeLevel {...props} dir="" />
    </div>
  );
}

function TreeLevel({ dir, ...props }: TreeViewProps & { dir: string }) {
  const { slug, revision } = props;
  const entries = useQuery(treeQuery(slug, revision, dir));
  if (entries.isPending) {
    return <Skeleton className="m-12" />;
  }
  if (entries.isError) {
    return (
      <ErrorBanner
        title="The tree could not load."
        message="The directory did not arrive."
        onRetry={() => void entries.refetch()}
      />
    );
  }
  return (
    <ul aria-label={dir === "" ? `Files at ${revision}` : dir}>
      {entries.data.map((entry) => (
        <TreeRow key={entry.path} entry={entry} {...props} />
      ))}
    </ul>
  );
}

function TreeRow({ entry, ...props }: TreeViewProps & { entry: TreeEntry }) {
  const { expanded, openPath, onToggle, onOpenFile } = props;
  const row =
    "flex w-full items-center gap-10 rounded-s px-10 py-6 text-left hover:bg-fill/55";
  if (!isOpenable(entry)) {
    return (
      <li>
        <div className={cn(row, "hover:bg-transparent")}>
          <span className="w-14 shrink-0" />
          <FileQuestion aria-hidden size={16} className="text-text-2" />
          <Mono className="min-w-0 flex-1 truncate text-14 text-text">
            {entry.name}
          </Mono>
          <span className="text-13 text-text-2">
            name is not UTF-8, not openable
          </span>
        </div>
      </li>
    );
  }
  if (entry.kind === "directory") {
    const open = expanded.has(entry.path);
    const FolderIcon = open ? FolderOpen : Folder;
    const Chevron = open ? ChevronDown : ChevronRight;
    return (
      <li>
        <button
          type="button"
          aria-expanded={open}
          onClick={() => {
            onToggle(entry.path);
          }}
          className={cn(row, focusRing)}
        >
          <Chevron
            aria-hidden
            size={14}
            className="w-14 shrink-0 text-text-2"
          />
          <FolderIcon aria-hidden size={16} className="shrink-0 text-text-2" />
          <Mono className="min-w-0 flex-1 truncate text-14 text-text">
            {entry.name}
          </Mono>
        </button>
        {open ? (
          <div className="pl-22">
            <TreeLevel {...props} dir={entry.path} />
          </div>
        ) : null}
      </li>
    );
  }
  return (
    <li>
      <button
        type="button"
        aria-current={openPath === entry.path ? "true" : undefined}
        onClick={() => {
          onOpenFile(entry.path);
        }}
        className={cn(
          row,
          openPath === entry.path && "bg-fill hover:bg-fill",
          focusRing,
        )}
      >
        <span className="w-14 shrink-0" />
        {entry.kind === "symlink" ? (
          <Link2 aria-hidden size={16} className="shrink-0 text-text-2" />
        ) : (
          <File aria-hidden size={16} className="shrink-0 text-text-2" />
        )}
        <Mono className="min-w-0 flex-1 truncate text-14 text-text">
          {entry.name}
          {symlinkLabel(entry) === null ? null : (
            <span className="text-text-2"> {symlinkLabel(entry)}</span>
          )}
        </Mono>
        {entry.size === null || entry.kind === "symlink" ? null : (
          <span className="shrink-0 text-13 text-text-2 tabular-nums">
            {formatSize(entry.size)}
          </span>
        )}
      </button>
    </li>
  );
}
