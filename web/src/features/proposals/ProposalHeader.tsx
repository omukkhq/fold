import { ActorText } from "@/people/ActorText";
import { segmentsText, type Segments } from "@/people/segments";
import { PersonName } from "@/people/PersonName";
import { ChevronRight, GitPullRequest } from "lucide-react";
import type { ProposalDetail } from "@/api/proposals";
import { HrefLink } from "@/app/HrefLink";
import { TAB_LIST, tabClasses } from "@/components/overlay/tabStyles";
import {
  SEGMENT_LIST,
  segmentClasses,
} from "@/components/segments/segmentStyles";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  claimText,
  currentVersion,
  filesText,
  reviewPill,
  versionText,
} from "./present";
import { ReviewPill } from "./ReviewPill";
import {
  changeOptions,
  detailHref,
  versionTabs,
  type DetailView,
} from "./view";

export function ProposalHeader({
  slug,
  proposal,
  view,
  main,
  revision,
}: {
  slug: string;
  proposal: ProposalDetail;
  view: DetailView;
  /** The head of main; null until the revisions arrive. */
  main: string | null;
  /** The revision an integrated proposal became; null until it is known. */
  revision: string | null;
}) {
  const summary = proposal.summary;
  const options = changeOptions(proposal, view);
  const current = currentVersion(proposal);
  const meta = [
    `proposal ${versionText(summary)}`,
    current
      ? main
        ? `base ${current.base} → main ${main}`
        : `base ${current.base}`
      : null,
    current ? filesText(current, proposal.totals) : null,
    summary.state === "open" ? claimText(summary) : null,
  ]
    .filter((m): m is string | Segments => m !== null)
    .map((m): Segments => (typeof m === "string" ? [m] : m));
  return (
    <header className="flex flex-col gap-10 px-24 pt-18">
      <nav aria-label="Breadcrumb">
        <ol className="flex items-center gap-8 text-bar">
          <li>
            <HrefLink
              href={`/p/${slug}/proposals`}
              className={cn(
                "rounded-s text-text-muted hover:text-text",
                focusRing,
              )}
            >
              Proposals
            </HrefLink>
          </li>
          <li className="flex items-center gap-8">
            <ChevronRight
              aria-hidden="true"
              size={13}
              className="text-text-faint"
            />
            <span aria-current="page" className="font-mono text-text">
              {summary.id}
            </span>
          </li>
        </ol>
      </nav>
      <div className="flex flex-wrap items-center gap-12">
        <h1 className="text-screen leading-snug font-semibold text-text">
          {summary.title}
        </h1>
        <ReviewPill
          pill={reviewPill(summary, {
            long: true,
            revision,
            conflictMain: proposal.conflict?.main_revision ?? null,
          })}
          large
        />
      </div>
      <div className="flex flex-wrap items-center gap-x-16 gap-y-4 text-bar text-text-muted">
        <span className="flex items-center gap-6 text-text">
          <PersonName name={summary.author} className="text-meta" />
        </span>
        <span>
          for{" "}
          <HrefLink
            href={`/p/${slug}/t/${summary.task}`}
            className={cn(
              "rounded-s font-mono hover:text-text hover:underline",
              focusRing,
            )}
          >
            {summary.task}
          </HrefLink>
        </span>
        {meta.map((item) => (
          <span key={segmentsText(item)}>
            <ActorText segments={item} />
          </span>
        ))}
      </div>
      <div className={cn(TAB_LIST, "justify-between")}>
        <nav aria-label="Versions" className="flex items-center gap-22">
          {versionTabs(proposal, view).map((tab) => (
            <HrefLink
              key={tab.key}
              href={detailHref(slug, summary.id, tab.search)}
              aria-current={tab.selected ? "page" : undefined}
              className={cn(tabClasses(tab.selected), focusRing)}
            >
              <GitPullRequest aria-hidden="true" size={15} />
              {tab.label}
            </HrefLink>
          ))}
        </nav>
        {options === null ? null : (
          <nav aria-label="Change" className={SEGMENT_LIST}>
            {options.map((option) => (
              <HrefLink
                key={option.key}
                href={detailHref(slug, summary.id, option.search)}
                aria-current={option.selected ? "page" : undefined}
                className={cn(segmentClasses(option.selected), focusRing)}
              >
                {option.label}
              </HrefLink>
            ))}
          </nav>
        )}
      </div>
    </header>
  );
}
