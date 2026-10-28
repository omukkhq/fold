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
    <div className="overflow-hidden rounded-l bg-surface inset-ring inset-ring-line">
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
    "flex w-full items-center gap-10 px-14 py-10 text-left hover:bg-sunken";
  if (!isOpenable(entry)) {
    return (
      <li className="border-b border-line last:border-b-0">
        <div className={cn(row, "hover:bg-transparent")}>
          <FileQuestion aria-hidden size={15} className="text-text-2" />
          <Mono className="flex-1 text-14 text-text">{entry.name}</Mono>
          <Mono className="text-13 text-text-2">
            name is not UTF-8, not openable
          </Mono>
        </div>
      </li>
    );
  }
  if (entry.kind === "directory") {
    const open = expanded.has(entry.path);
    const FolderIcon = open ? FolderOpen : Folder;
    const Chevron = open ? ChevronDown : ChevronRight;
    return (
      <li className="border-b border-line last:border-b-0">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => {
            onToggle(entry.path);
          }}
          className={cn(row, focusRing)}
        >
          <Chevron aria-hidden size={13} className="text-text-2" />
          <FolderIcon aria-hidden size={15} className="text-accent" />
          <Mono className="text-14 text-text">{entry.name}</Mono>
        </button>
        {open ? (
          <div className="ml-24 border-l border-line">
            <TreeLevel {...props} dir={entry.path} />
          </div>
        ) : null}
      </li>
    );
  }
  return (
    <li className="border-b border-line last:border-b-0">
      <button
        type="button"
        aria-current={openPath === entry.path ? "true" : undefined}
        onClick={() => {
          onOpenFile(entry.path);
        }}
        className={cn(
          row,
          openPath === entry.path && "bg-accent-soft",
          focusRing,
        )}
      >
        {entry.kind === "symlink" ? (
          <Link2 aria-hidden size={15} className="text-text-2" />
        ) : (
          <File aria-hidden size={15} className="text-text-2" />
        )}
        <Mono className="flex-1 text-14 text-text">{entry.name}</Mono>
        {symlinkLabel(entry) === null ? null : (
          <Mono className="text-13 text-text-2">{symlinkLabel(entry)}</Mono>
        )}
        {entry.size === null || entry.kind === "symlink" ? null : (
          <Mono className="text-13 text-text-2">{formatSize(entry.size)}</Mono>
        )}
      </button>
    </li>
  );
}
