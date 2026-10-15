import { ChevronRight, GitCompare, GitPullRequest } from "lucide-react";
import type { ProposalDetail } from "@/api/proposals";
import { HrefLink } from "@/app/HrefLink";
import { Avatar } from "@/components/avatar/Avatar";
import { TAB_LIST, tabClasses } from "@/components/overlay/tabStyles";
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
import { detailHref, versionTabs, type DetailView } from "./view";

export function ProposalHeader({
  slug,
  proposal,
  view,
}: {
  slug: string;
  proposal: ProposalDetail;
  view: DetailView;
}) {
  const current = currentVersion(proposal);
  const meta = [
    `proposal ${versionText(proposal)}`,
    current ? `base ${current.base} → main ${proposal.main}` : null,
    current ? filesText(current) : null,
    proposal.state === "open" ? claimText(proposal) : null,
  ].filter((m): m is string => m !== null);
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
              {proposal.id}
            </span>
          </li>
        </ol>
      </nav>
      <div className="flex flex-wrap items-center gap-12">
        <h1 className="text-screen leading-snug font-semibold text-text">
          {proposal.title}
        </h1>
        <ReviewPill
          pill={reviewPill(proposal, true, proposal.conflict?.main ?? null)}
          large
        />
      </div>
      <div className="flex flex-wrap items-center gap-x-16 gap-y-4 text-bar text-text-muted">
        <span className="flex items-center gap-6 text-text">
          <Avatar name={proposal.author} />
          <span className="font-mono text-code">{proposal.author}</span>
        </span>
        <span>
          for{" "}
          <HrefLink
            href={`/p/${slug}/t/${proposal.task}`}
            className={cn(
              "rounded-s font-mono hover:text-text hover:underline",
              focusRing,
            )}
          >
            {proposal.task}
          </HrefLink>
        </span>
        {meta.map((item) => (
          <span key={item}>{item}</span>
        ))}
      </div>
      <nav aria-label="Versions" className={TAB_LIST}>
        {versionTabs(proposal, view).map((tab) => {
          const Icon = tab.compare ? GitCompare : GitPullRequest;
          return (
            <HrefLink
              key={tab.key}
              href={detailHref(slug, proposal.id, tab.search)}
              aria-current={tab.selected ? "page" : undefined}
              className={cn(tabClasses(tab.selected), focusRing)}
            >
              <Icon aria-hidden="true" size={15} />
              {tab.label}
            </HrefLink>
          );
        })}
      </nav>
    </header>
  );
}
