import { ActorText } from "@/people/ActorText";
import { segmentsText, type Segments } from "@/people/segments";
import { PersonName } from "@/people/PersonName";
import { ChevronRight, Hourglass } from "lucide-react";
import type { ProposalDetail } from "@/api/proposals";
import { HrefLink } from "@/app/HrefLink";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  claimText,
  currentVersion,
  filesText,
  reviewPill,
  versionText,
} from "./present";
import { planWait } from "./planWait";
import { ReviewPill } from "./ReviewPill";
import { projectPath } from "@/routes/paths";

export function ProposalHeader({
  slug,
  proposal,
  main,
  revision,
  batch,
  now,
}: {
  slug: string;
  proposal: ProposalDetail;
  now: number;
  /** The head of main; null until the revisions arrive. */
  main: string | null;
  /** The revision an integrated proposal became; null until it is known. */
  revision: string | null;
  /** The review batch the proposal integrated with; null for one integrated on its own. */
  batch: string | null;
}) {
  const summary = proposal.summary;
  const current = currentVersion(proposal);
  const wait = planWait(proposal.review_plan, summary.id, now);
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
    <header className="flex flex-col gap-10 px-16 pt-18 md:px-24">
      <nav aria-label="Breadcrumb">
        <ol className="flex items-center gap-8 text-bar">
          <li>
            <HrefLink
              href={projectPath(slug, "proposals")}
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
            batch,
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
            href={projectPath(slug, "t", summary.task)}
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
      {wait === null ? null : (
        <p
          data-plan-wait={wait.state}
          className="flex items-start gap-6 self-start rounded-s bg-secondary-soft px-10 py-5 text-meta leading-normal text-secondary-text"
        >
          <Hourglass aria-hidden="true" size={13} className="mt-2 shrink-0" />
          <span>
            <span className="font-semibold">{wait.title}</span> · {wait.detail}
          </span>
        </p>
      )}
    </header>
  );
}
