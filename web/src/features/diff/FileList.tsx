import type { ComparedFile } from "@/api/diff";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { changeBadge, lineCounts } from "./present";

interface FileListProps {
  files: readonly ComparedFile[];
  selectedPath: string | null;
  onSelectFile: (path: string) => void;
  /** An overline above the files; the proposal screen draws one, the diff screen does not. */
  title?: string;
}

export function FileList({
  files,
  selectedPath,
  onSelectFile,
  title,
}: FileListProps) {
  return (
    <nav
      aria-label="Changed files"
      className="flex w-file-list shrink-0 flex-col gap-2"
    >
      {title ? (
        <h2 className="text-overline leading-snug font-semibold text-primary-text uppercase">
          {title}
        </h2>
      ) : null}
      <ul className="flex flex-col gap-2">
        {files.map((file) => {
          const badge = changeBadge(file.change);
          const counts = lineCounts(file);
          const current = file.path === selectedPath;
          return (
            <li key={file.path}>
              <button
                type="button"
                aria-current={current ? "true" : undefined}
                onClick={() => {
                  onSelectFile(file.path);
                }}
                className={cn(
                  "flex w-full items-center gap-10 rounded-s px-8 py-6 text-left hover:bg-sunken",
                  current && "bg-sunken",
                  focusRing,
                )}
              >
                <span
                  className={cn(
                    "flex size-18 shrink-0 items-center justify-center rounded-s font-mono text-caption font-bold",
                    badge.classes,
                  )}
                >
                  <span aria-hidden="true">{badge.letter}</span>
                  <span className="sr-only">{badge.label}</span>
                </span>
                <span className="min-w-0 flex-1 truncate font-mono text-label text-text">
                  {file.path}
                </span>
                {file.binary ? (
                  <span className="font-mono text-meta text-text-muted">
                    bin
                  </span>
                ) : null}
                {counts.added ? (
                  <span className="font-mono text-meta text-primary-text">
                    {counts.added}
                  </span>
                ) : null}
                {counts.removed ? (
                  <span className="font-mono text-meta text-danger">
                    {counts.removed}
                  </span>
                ) : null}
              </button>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}
