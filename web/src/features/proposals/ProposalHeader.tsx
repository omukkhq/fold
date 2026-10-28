import { ChevronRight, Hourglass } from "lucide-react";
import type { ProposalDetail } from "@/api/proposals";
import { HrefLink } from "@/app/HrefLink";
import { Id } from "@/components/id/Id";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { PersonName } from "@/people/PersonName";
import { projectPath } from "@/routes/paths";
import { IdLink } from "./IdLink";
import { planWait } from "./planWait";
import { reviewPill, versionText } from "./present";
import { ReviewPill } from "./ReviewPill";

/** The proposal's title with its one state chip and three facts: the author, the task and the version. The base, the size and the review claim are on the About card. */
export function ProposalHeader({
  slug,
  proposal,
  revision,
  batch,
  now,
}: {
  slug: string;
  proposal: ProposalDetail;
  now: number;
  /** The revision an integrated proposal became; null until it is known. */
  revision: string | null;
  /** The review batch the proposal integrated with; null for one integrated on its own. */
  batch: string | null;
}) {
  const summary = proposal.summary;
  const wait = planWait(proposal.review_plan, summary.id, now);
  return (
    <header className="flex flex-col gap-8 px-16 pt-20 md:px-24">
      <nav aria-label="Breadcrumb">
        <ol className="flex items-center gap-6 text-14">
          <li>
            <HrefLink
              href={projectPath(slug, "proposals")}
              className={cn("rounded-s text-text-2 hover:text-text", focusRing)}
            >
              Proposals
            </HrefLink>
          </li>
          <li className="flex items-center gap-6">
            <ChevronRight
              aria-hidden="true"
              size={14}
              className="text-text-2"
            />
            <Id aria-current="page" className="font-medium text-text">
              {summary.id}
            </Id>
          </li>
        </ol>
      </nav>
      <div className="flex flex-wrap items-center gap-x-14 gap-y-8">
        <h1 className="min-w-0 text-24 leading-tight font-semibold wrap-anywhere text-text">
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
      <p className="text-14 text-text-2">
        <PersonName name={summary.author} /> · for{" "}
        <IdLink
          href={projectPath(slug, "t", summary.task)}
          className="text-text"
        >
          {summary.task}
        </IdLink>{" "}
        · proposal {versionText(summary)}
      </p>
      {wait === null ? null : (
        <p
          data-plan-wait={wait.state}
          className="flex max-w-measure items-start gap-8 self-start rounded-s bg-warn-soft px-12 py-6 text-14 leading-normal text-warn"
        >
          <Hourglass aria-hidden="true" size={14} className="mt-4 shrink-0" />
          <span>
            <span className="font-semibold">{wait.title}</span> · {wait.detail}
          </span>
        </p>
      )}
    </header>
  );
}
