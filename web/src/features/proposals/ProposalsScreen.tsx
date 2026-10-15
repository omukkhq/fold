import { PersonName } from "@/people/PersonName";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { Check, GitMerge, GitPullRequest, UserRoundCheck } from "lucide-react";
import { proposalsQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { Badge } from "@/components/badge/Badge";
import { Button } from "@/components/button/Button";
import { Menu } from "@/components/overlay/Menu";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { Skeleton } from "@/components/states/Skeleton";
import { revisionsQuery } from "@/api/repositoryQueries";
import { formatSince } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { useNow } from "@/live/useNow";
import {
  STATES,
  claimLabel,
  countByState,
  filterProposals,
  isNarrowed,
  listHref,
  shownState,
  type ClaimFilter,
  type ListSearch,
} from "./filters";
import { integratedRevisions } from "./integration";
import { reviewPill, versionText, waitingOn } from "./present";
import { ReviewPill, WaitingChip } from "./ReviewPill";

interface ProposalsScreenProps {
  slug: string;
  search: ListSearch;
  onSearch: (next: ListSearch) => void;
}

const STATE_LABELS = {
  open: "Open",
  integrated: "Integrated",
  closed: "Closed",
} as const;

const HEAD =
  "px-6 py-9 text-left text-overline leading-snug font-semibold whitespace-nowrap text-text-muted uppercase first:pl-16 last:pr-16";
const CELL = "px-6 py-12 align-middle first:pl-16 last:pr-16";
const MONO = "font-mono text-code whitespace-nowrap text-text-muted";

const CLAIMS: readonly (ClaimFilter | undefined)[] = [
  undefined,
  "claimed",
  "unclaimed",
];

export function ProposalsScreen({
  slug,
  search,
  onSearch,
}: ProposalsScreenProps) {
  const { data } = useSuspenseQuery(proposalsQuery(slug));
  const now = useNow(true, 30_000);
  const state = shownState(search);
  const counts = countByState(data);
  const rows = filterProposals(data, search);
  const revisions = useQuery({
    ...revisionsQuery(slug),
    enabled: state === "integrated",
  });
  const integrated = integratedRevisions(revisions.data?.items ?? []);

  return (
    <div className="flex flex-col gap-16 px-24 pt-20 pb-24">
      <div className="flex flex-wrap items-end justify-between gap-12">
        <div className="flex flex-col gap-4">
          <h1 className="text-screen leading-snug font-semibold text-text">
            Proposals
          </h1>
          <p className="text-body text-text-muted">
            Proposals waiting on reviewers, waiting on authors, and recently
            integrated.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-8">
          <Menu
            align="end"
            trigger={
              <Button icon={<UserRoundCheck aria-hidden="true" size={15} />}>
                {claimLabel(search.claim)}
              </Button>
            }
            items={CLAIMS.map((claim) => ({
              label: claimLabel(claim),
              icon: claim === search.claim ? Check : undefined,
              onSelect: () => {
                onSearch({ ...search, claim });
              },
            }))}
          />
          <Button
            aria-pressed={search.conflicted === true}
            icon={<GitMerge aria-hidden="true" size={15} />}
            className={cn(
              search.conflicted &&
                "bg-danger-soft text-danger inset-ring-danger",
            )}
            onClick={() => {
              onSearch({
                ...search,
                conflicted: search.conflicted ? undefined : true,
              });
            }}
          >
            Conflicted
          </Button>
          <nav
            aria-label="Proposal state"
            className="flex gap-2 rounded-m bg-sunken p-3"
          >
            {STATES.map((s) => {
              const current = s === state;
              return (
                <HrefLink
                  key={s}
                  href={listHref(slug, { ...search, state: s })}
                  aria-current={current ? "page" : undefined}
                  className={cn(
                    "rounded-s px-12 py-5 text-bar leading-snug",
                    current
                      ? "bg-surface font-semibold text-text inset-ring inset-ring-border-subtle"
                      : "font-medium text-text-muted hover:text-text",
                    focusRing,
                  )}
                >
                  {s === "open"
                    ? `${STATE_LABELS[s]} · ${String(counts.open)}`
                    : STATE_LABELS[s]}
                </HrefLink>
              );
            })}
          </nav>
        </div>
      </div>

      {rows.length === 0 ? (
        <EmptyPanel
          icon={GitPullRequest}
          title={
            data.length === 0
              ? "No proposals yet"
              : isNarrowed(search)
                ? "No proposal matches"
                : `No ${STATE_LABELS[state].toLowerCase()} proposals`
          }
        >
          {data.length === 0
            ? "A proposal appears here when an agent opens one with create_proposal."
            : isNarrowed(search)
              ? "Change or clear the filters to see more proposals."
              : "Pick another state to see more proposals."}
        </EmptyPanel>
      ) : (
        <div className="overflow-x-auto rounded-l border border-border-subtle bg-surface">
          <table className="w-full border-collapse">
            <thead className="bg-bg">
              <tr className="border-b border-border-subtle">
                <th className={HEAD}>Id</th>
                <th className={cn(HEAD, "w-full")}>Title</th>
                <th className={HEAD}>Task</th>
                <th className={HEAD}>Author</th>
                <th className={HEAD}>Proposal</th>
                <th className={HEAD}>Waiting on</th>
                <th className={HEAD}>Review</th>
                <th className={HEAD}>Updated</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((p) => {
                const waiting = waitingOn(p);
                return (
                  <tr
                    key={p.id}
                    className="border-b border-border-subtle last:border-b-0 hover:bg-bg"
                  >
                    <td className={cn(CELL, MONO)}>{p.id}</td>
                    <td className={CELL}>
                      <span className="flex flex-wrap items-center gap-8">
                        <HrefLink
                          href={`/p/${slug}/proposals/${p.id}`}
                          className={cn(
                            "rounded-s text-body leading-snug font-medium text-text hover:underline",
                            focusRing,
                          )}
                        >
                          {p.title}
                        </HrefLink>
                        {p.human_review ? (
                          <Badge tone="human-review">
                            {p.human_review.reviewer
                              ? `Human review · ${p.human_review.reviewer}`
                              : "Human review"}
                          </Badge>
                        ) : null}
                      </span>
                    </td>
                    <td className={cn(CELL, MONO)}>{p.task}</td>
                    <td className={cn(CELL, MONO)}>
                      <PersonName name={p.author} chip={false} />
                    </td>
                    <td className={cn(CELL, MONO)}>{versionText(p)}</td>
                    <td className={CELL}>
                      {waiting ? (
                        <WaitingChip waiting={waiting} />
                      ) : (
                        <span className="text-meta text-text-faint">—</span>
                      )}
                    </td>
                    <td className={CELL}>
                      <span className="flex flex-col items-start gap-4">
                        <ReviewPill
                          pill={reviewPill(p, {
                            revision: integrated.get(p.id) ?? null,
                          })}
                          className="whitespace-nowrap"
                        />
                        <span className="font-mono text-caption whitespace-nowrap text-text-muted">
                          {p.review_claim
                            ? `claimed by ${p.review_claim}`
                            : "unclaimed"}
                        </span>
                      </span>
                    </td>
                    <td
                      className={cn(
                        CELL,
                        "text-code whitespace-nowrap text-text-muted",
                      )}
                    >
                      {formatSince(now, p.updated_at)}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

export function ProposalsPending() {
  return (
    <div className="flex flex-col gap-16 px-24 pt-20 pb-24">
      <Skeleton />
    </div>
  );
}
