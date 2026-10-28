import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { Check, ChevronDown, Filter, GitPullRequest, X } from "lucide-react";
import { proposalsQuery, proposalsWaitingQuery } from "@/api/queries";
import { revisionsQuery } from "@/api/repositoryQueries";
import { HrefLink } from "@/app/HrefLink";
import { FILTER_COUNT_CLASSES } from "@/components/badge/chips";
import { Button } from "@/components/button/Button";
import { Id } from "@/components/id/Id";
import { menuContent, menuItem } from "@/components/overlay/menuStyles";
import {
  SEGMENT_LIST,
  segmentClasses,
} from "@/components/segments/segmentStyles";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { Skeleton } from "@/components/states/Skeleton";
import {
  TABLE_CELL,
  TABLE_HEAD,
  TABLE_LINK_ROW,
} from "@/components/table/tableStyles";
import { formatSince } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { useNow } from "@/live/useNow";
import { ActorText } from "@/people/ActorText";
import { PersonName } from "@/people/PersonName";
import { projectPath } from "@/routes/paths";
import {
  STATES,
  activeFilters,
  claimLabel,
  countByState,
  filterProposals,
  isNarrowed,
  listHref,
  menuCounts,
  shownState,
  type ClaimFilter,
  type ListSearch,
} from "./filters";
import { IdLink } from "@/components/id/IdLink";
import { integratedRevisions } from "./integration";
import { listMeta, listPill } from "./present";
import { ReviewPill } from "./ReviewPill";

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

const CLAIMS: readonly (ClaimFilter | undefined)[] = [
  undefined,
  "claimed",
  "unclaimed",
];

const MENU_HEAD =
  "px-10 pt-8 pb-4 font-title text-13 font-semibold text-text-2";
const MENU_ROW = cn(menuItem, "gap-10");

function Tick() {
  return (
    <span className="inline-flex size-16 shrink-0 items-center justify-center">
      <DropdownMenu.ItemIndicator>
        <Check aria-hidden="true" size={16} />
      </DropdownMenu.ItemIndicator>
    </span>
  );
}

/** One Filter menu for the toggles and the claim filter; the state segment stays beside it. The menu stays open while the reader picks, so several filters take one visit. */
function FilterMenu({
  search,
  counts,
  onSearch,
}: {
  search: ListSearch;
  counts: { waiting: number; conflicted: number };
  onSearch: (next: ListSearch) => void;
}) {
  const active = activeFilters(search).length;
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild>
        <Button icon={<Filter aria-hidden="true" size={16} />}>
          Filter
          {active > 0 ? (
            <span className={FILTER_COUNT_CLASSES}>
              {active}
              <span className="sr-only"> active</span>
            </span>
          ) : null}
          <ChevronDown aria-hidden="true" size={14} className="text-text-2" />
        </Button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          sideOffset={8}
          className={cn(menuContent, "w-menu max-w-(--width-popover-max)")}
        >
          <DropdownMenu.Label className={MENU_HEAD}>
            Show only
          </DropdownMenu.Label>
          <DropdownMenu.CheckboxItem
            className={MENU_ROW}
            checked={search.waiting === true}
            onCheckedChange={(checked) => {
              onSearch({ ...search, waiting: checked ? true : undefined });
            }}
            onSelect={(event) => {
              event.preventDefault();
            }}
          >
            <Tick />
            Waits for you{" "}
            <span className="ml-auto text-13 text-text-2 tabular-nums">
              {counts.waiting}
            </span>
          </DropdownMenu.CheckboxItem>
          <DropdownMenu.CheckboxItem
            className={MENU_ROW}
            checked={search.conflicted === true}
            onCheckedChange={(checked) => {
              onSearch({ ...search, conflicted: checked ? true : undefined });
            }}
            onSelect={(event) => {
              event.preventDefault();
            }}
          >
            <Tick />
            Conflicted{" "}
            <span className="ml-auto text-13 text-text-2 tabular-nums">
              {counts.conflicted}
            </span>
          </DropdownMenu.CheckboxItem>
          <DropdownMenu.Label className={MENU_HEAD}>
            Review claim
          </DropdownMenu.Label>
          <DropdownMenu.RadioGroup
            value={search.claim ?? "any"}
            onValueChange={(value) => {
              onSearch({
                ...search,
                claim: CLAIMS.find((claim) => claim === value),
              });
            }}
          >
            {CLAIMS.map((claim) => (
              <DropdownMenu.RadioItem
                key={claim ?? "any"}
                value={claim ?? "any"}
                className={MENU_ROW}
                onSelect={(event) => {
                  event.preventDefault();
                }}
              >
                <Tick />
                {claimLabel(claim)}
              </DropdownMenu.RadioItem>
            ))}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

const CELL = cn(TABLE_CELL, "px-14 py-12");

export function ProposalsScreen({
  slug,
  search,
  onSearch,
}: ProposalsScreenProps) {
  const { data } = useSuspenseQuery(proposalsQuery(slug));
  const { data: waitingForYou = [] } = useQuery(proposalsWaitingQuery(slug));
  const now = useNow(true, 30_000);
  const state = shownState(search);
  const counts = countByState(data);
  const rows = filterProposals(data, search, waitingForYou);
  const revisions = useQuery({
    ...revisionsQuery(slug),
    enabled: state === "integrated",
  });
  const integrated = integratedRevisions(revisions.data?.items ?? []);
  const active = activeFilters(search);

  return (
    <div className="flex flex-col gap-16 px-16 pt-20 pb-28 md:px-24">
      <div className="flex flex-wrap items-end justify-between gap-x-20 gap-y-12">
        <div className="flex flex-col gap-4">
          <h1 className="text-24 leading-tight font-semibold text-text">
            Proposals
          </h1>
          <p className="max-w-measure text-15 text-text-2">
            Proposals waiting on reviewers, waiting on authors, and recently
            integrated.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-8">
          <nav aria-label="Proposal state" className={SEGMENT_LIST}>
            {STATES.map((s) => {
              const current = s === state;
              return (
                <HrefLink
                  key={s}
                  href={listHref(slug, { ...search, state: s })}
                  aria-current={current ? "page" : undefined}
                  className={cn(segmentClasses(current), focusRing)}
                >
                  {STATE_LABELS[s]}
                  {s === "open" ? (
                    <>
                      {" "}
                      <span className="font-body text-13 font-medium text-text-2 tabular-nums">
                        {counts.open}
                      </span>
                    </>
                  ) : null}
                </HrefLink>
              );
            })}
          </nav>
          <FilterMenu
            search={search}
            counts={menuCounts(data, search, waitingForYou)}
            onSearch={onSearch}
          />
        </div>
      </div>

      {active.length === 0 ? null : (
        <div className="flex flex-wrap items-center gap-8 text-14">
          <span className="text-text-2">Filters:</span>
          {active.map((filter) => (
            <button
              key={filter.key}
              type="button"
              aria-label={`Remove the filter ${filter.label}`}
              onClick={() => {
                onSearch(filter.without);
              }}
              className={cn(
                "inline-flex items-center gap-6 rounded-pill bg-accent-soft py-4 pr-8 pl-12 text-14 font-semibold text-accent",
                focusRing,
              )}
            >
              {filter.label}
              <X aria-hidden="true" size={14} />
            </button>
          ))}
        </div>
      )}

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
        <div className="overflow-x-auto rounded-m bg-surface max-md:overflow-visible max-md:bg-transparent">
          {/* On a phone each row becomes a block of two lines: the title, then the state chip and the time. */}
          <table className="w-full border-collapse max-md:block">
            <thead className="max-md:sr-only">
              <tr>
                <th className={cn(TABLE_HEAD, "w-full px-14 pt-10 pb-6")}>
                  Proposal
                </th>
                <th className={cn(TABLE_HEAD, "px-14 pt-10 pb-6")}>Author</th>
                <th className={cn(TABLE_HEAD, "px-14 pt-10 pb-6")}>State</th>
                <th className={cn(TABLE_HEAD, "px-14 pt-10 pb-6 text-right")}>
                  Updated
                </th>
              </tr>
            </thead>
            <tbody className="max-md:flex max-md:flex-col max-md:gap-8">
              {rows.map((p) => (
                <tr
                  key={p.id}
                  className={cn(
                    TABLE_LINK_ROW,
                    "max-md:grid max-md:grid-cols-[minmax(0,1fr)_auto] max-md:items-center max-md:gap-x-10 max-md:gap-y-6 max-md:rounded-m max-md:bg-surface max-md:px-14 max-md:py-12 max-md:even:bg-surface max-md:hover:bg-surface",
                  )}
                >
                  <td className={cn(CELL, "max-md:col-span-2 max-md:p-0")}>
                    <div className="flex min-w-0 flex-col gap-2">
                      <span className="flex min-w-0 items-baseline gap-12">
                        <Id className="min-w-32 shrink-0 text-14 text-text-2">
                          {p.id}
                        </Id>
                        <HrefLink
                          href={projectPath(slug, "proposals", p.id)}
                          className={cn(
                            "min-w-0 rounded-s text-15 leading-tight font-medium wrap-anywhere text-text hover:underline",
                            focusRing,
                          )}
                        >
                          {p.title}
                        </HrefLink>
                      </span>
                      <span className="pl-44 text-13 text-text-2 max-md:hidden">
                        for{" "}
                        <IdLink href={projectPath(slug, "t", p.task)}>
                          {p.task}
                        </IdLink>{" "}
                        · <ActorText segments={listMeta(p)} />
                      </span>
                    </div>
                  </td>
                  <td className={cn(CELL, "whitespace-nowrap max-md:hidden")}>
                    <PersonName name={p.author} />
                  </td>
                  <td className={cn(CELL, "max-md:p-0")}>
                    <ReviewPill
                      pill={listPill(p, {
                        revision: integrated.get(p.id) ?? null,
                        waitsForYou: waitingForYou.includes(p.id),
                      })}
                      className="md:whitespace-nowrap"
                    />
                  </td>
                  <td
                    className={cn(
                      CELL,
                      "text-right text-13 whitespace-nowrap text-text-2 tabular-nums max-md:p-0",
                    )}
                  >
                    {formatSince(now, p.updated_at)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

export function ProposalsPending() {
  return (
    <div className="flex flex-col gap-16 px-16 pt-20 pb-28 md:px-24">
      <Skeleton />
    </div>
  );
}
