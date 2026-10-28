import { ArrowRight, ChevronRight } from "lucide-react";
import type { ComparedFile, Comparison } from "@/api/diff";
import { HrefLink } from "@/app/HrefLink";
import { formatAsOf } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  comparisonCrumbs,
  comparisonNote,
  comparisonSides,
  comparisonTitle,
  namesSides,
  stateLabel,
} from "./present";

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
  const [older, newer] = comparisonSides(comparison);
  return (
    <header className="flex flex-col gap-6 px-16 pt-18 pb-14 md:px-24">
      <nav aria-label="Breadcrumb">
        <ol className="flex flex-wrap items-center gap-8 text-14 text-text-2">
          {crumbs.map((crumb, index) => (
            <li key={crumb.label} className="flex items-center gap-8">
              {index > 0 ? <ChevronRight aria-hidden="true" size={14} /> : null}
              {crumb.href ? (
                <HrefLink
                  href={crumb.href}
                  className={cn("rounded-s hover:text-text", focusRing)}
                >
                  {crumb.label}
                </HrefLink>
              ) : (
                <span
                  aria-current="page"
                  className="whitespace-nowrap text-text tabular-nums"
                >
                  {crumb.label}
                </span>
              )}
            </li>
          ))}
        </ol>
      </nav>
      <div className="flex flex-wrap items-center gap-x-12 gap-y-6">
        <h1 className="text-24 leading-tight font-semibold text-text">
          {comparisonTitle(comparison)}
        </h1>
        {readAt === null ? null : (
          <span className="inline-flex items-center gap-6 rounded-pill bg-accent-soft px-9 py-2 text-13 font-medium text-accent">
            <span
              aria-hidden="true"
              className="size-7 rounded-full bg-accent"
            />
            live · read {formatAsOf(now, readAt)}
          </span>
        )}
      </div>
      <p className="flex flex-wrap items-center gap-x-8 gap-y-2 text-13 text-text-2 tabular-nums">
        {namesSides(comparison) ? (
          <>
            <span>{stateLabel(older, comparison)}</span>
            <ArrowRight aria-hidden="true" size={14} />
            <span className="sr-only">to</span>
            <span>{stateLabel(newer, comparison)}</span>
            <span aria-hidden="true">·</span>
          </>
        ) : null}
        <span>{comparisonNote(comparison, files)}</span>
      </p>
    </header>
  );
}
