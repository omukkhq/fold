import {
  Bookmark,
  ChevronRight,
  FilePen,
  FolderTree,
  GitCommitHorizontal,
  GitCommitVertical,
  type LucideIcon,
} from "lucide-react";
import type {
  ComparedFile,
  ComparedState,
  Comparison,
  StateKind,
} from "@/api/diff";
import { HrefLink } from "@/app/HrefLink";
import { formatAsOf } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  comparisonCrumbs,
  comparisonNote,
  comparisonTitle,
  stateLabel,
} from "./present";

const STATE_ICONS: Record<StateKind, LucideIcon> = {
  revision: GitCommitHorizontal,
  changeset: GitCommitVertical,
  checkpoint: Bookmark,
  workspace: FilePen,
  tree: FolderTree,
};

function StateChip({
  state,
  comparison,
}: {
  state: ComparedState;
  comparison: Comparison;
}) {
  const Icon = STATE_ICONS[state.kind];
  return (
    <span className="inline-flex items-center gap-6 rounded-m bg-surface px-14 py-7 text-body leading-snug font-medium text-text inset-ring inset-ring-border">
      <Icon aria-hidden="true" size={15} className="text-text-muted" />
      {stateLabel(state, comparison)}
    </span>
  );
}

interface DiffHeaderProps {
  slug: string;
  comparison: Comparison;
  files: readonly ComparedFile[];
  now: number;
  /** When the working files were last read; null for a comparison of recorded states. */
  readAt: number | null;
}

export function DiffHeader({
  slug,
  comparison,
  files,
  now,
  readAt,
}: DiffHeaderProps) {
  const crumbs = comparisonCrumbs(slug, comparison);
  return (
    <header className="flex flex-col gap-8 px-24 pt-18 pb-12">
      <nav aria-label="Breadcrumb">
        <ol className="flex items-center gap-8 text-bar">
          {crumbs.map((crumb, index) => (
            <li key={crumb.label} className="flex items-center gap-8">
              {index > 0 ? (
                <ChevronRight
                  aria-hidden="true"
                  size={13}
                  className="text-text-faint"
                />
              ) : null}
              {crumb.href ? (
                <HrefLink
                  href={crumb.href}
                  className={cn(
                    "rounded-s text-text-muted hover:text-text",
                    focusRing,
                  )}
                >
                  {crumb.label}
                </HrefLink>
              ) : (
                <span
                  aria-current="page"
                  className={cn("text-text", crumb.mono && "font-mono")}
                >
                  {crumb.label}
                </span>
              )}
            </li>
          ))}
        </ol>
      </nav>
      <div className="flex items-center gap-12">
        <h1 className="text-screen leading-snug font-semibold text-text">
          {comparisonTitle(comparison)}
        </h1>
        {readAt === null ? null : (
          <span className="inline-flex items-center gap-6 rounded-s bg-primary-soft px-8 py-2 text-caption font-medium text-primary-strong">
            <span
              aria-hidden="true"
              className="size-7 rounded-full bg-primary"
            />
            live · read {formatAsOf(now, readAt)}
          </span>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-10">
        <StateChip state={comparison.to} comparison={comparison} />
        <span className="text-body text-text-muted">vs</span>
        <StateChip state={comparison.from} comparison={comparison} />
        <span className="text-bar text-text-muted">
          {comparisonNote(comparison, files)}
        </span>
      </div>
    </header>
  );
}
