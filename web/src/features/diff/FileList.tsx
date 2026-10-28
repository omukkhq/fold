import { TriangleAlert } from "lucide-react";
import type { ComparedFile, FileWarning } from "@/api/diff";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { changeBadge, lineCounts, warningMark } from "./present";

interface FileListProps {
  files: readonly ComparedFile[];
  /** Working files that could not be read, listed after the changed files. */
  warnings?: readonly FileWarning[];
  selectedPath: string | null;
  onSelectFile: (path: string) => void;
  /** An overline above the files; the proposal screen draws one, the diff screen does not. */
  title?: string;
}

export function FileList({
  files,
  warnings = [],
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
        <h2 className="font-title text-13 leading-tight font-semibold text-text-2">
          {title}
        </h2>
      ) : null}
      <ul className="flex flex-col gap-2">
        {files.map((file) => {
          const badge = changeBadge(file.change);
          const counts = lineCounts(file);
          const current = file.path === selectedPath;
          const warned = warnings.find((w) => w.path === file.path);
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
                    "flex size-18 shrink-0 items-center justify-center rounded-s font-mono text-13 font-bold",
                    badge.classes,
                  )}
                >
                  <span aria-hidden="true">{badge.letter}</span>
                  <span className="sr-only">{badge.label}</span>
                </span>
                <span className="min-w-0 flex-1 truncate font-mono text-13 text-text">
                  {file.path}
                </span>
                {warned ? (
                  <span className="font-mono text-13 text-warn">
                    {warningMark(warned)}
                  </span>
                ) : null}
                {file.binary ? (
                  <span className="font-mono text-13 text-text-2">bin</span>
                ) : null}
                {counts.added ? (
                  <span className="font-mono text-13 text-accent">
                    {counts.added}
                  </span>
                ) : null}
                {counts.removed ? (
                  <span className="font-mono text-13 text-bad">
                    {counts.removed}
                  </span>
                ) : null}
              </button>
            </li>
          );
        })}
        {warnings
          .filter((warning) => !files.some((f) => f.path === warning.path))
          .map((warning) => {
            const current = warning.path === selectedPath;
            return (
              <li key={`warning:${warning.path}`}>
                <button
                  type="button"
                  aria-current={current ? "true" : undefined}
                  onClick={() => {
                    onSelectFile(warning.path);
                  }}
                  className={cn(
                    "flex w-full items-center gap-10 rounded-s px-8 py-6 text-left hover:bg-sunken",
                    current && "bg-sunken",
                    focusRing,
                  )}
                >
                  <span className="flex size-18 shrink-0 items-center justify-center rounded-s bg-warn-soft text-warn">
                    <TriangleAlert aria-hidden="true" size={12} />
                  </span>
                  <span className="min-w-0 flex-1 truncate font-mono text-13 text-text">
                    {warning.path}
                  </span>
                  <span className="font-mono text-13 text-text-2">
                    {warningMark(warning)}
                  </span>
                </button>
              </li>
            );
          })}
      </ul>
    </nav>
  );
}
