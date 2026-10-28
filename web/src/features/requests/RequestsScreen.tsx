import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { useQuery } from "@tanstack/react-query";
import {
  Check,
  ChevronDown,
  Inbox,
  Plus,
  RefreshCw,
  UserRound,
} from "lucide-react";
import { useState, type ReactNode } from "react";
import type { RequestSummary } from "@/api/requests";
import { requestsQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { menuContent, menuItem } from "@/components/overlay/menuStyles";
import { FailurePanel } from "@/components/states/FailurePanel";
import {
  TABLE_CELL,
  TABLE_HEAD,
  TABLE_LINK_ROW,
  TABLE_ROW,
} from "@/components/table/tableStyles";
import { formatAge } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { useMediaQuery } from "@/lib/pageState";
import { useNow } from "@/live/useNow";
import { PersonName } from "@/people/PersonName";
import { projectPath } from "@/routes/paths";
import { describeFailure } from "./errors";
import {
  KINDS,
  countByState,
  filterRequests,
  isNarrowed,
  listHref,
  toggleMine,
  visibleStates,
  withKind,
  type ListSearch,
  type StateCounts,
} from "./filters";
import { NewRequestDialog } from "./NewRequestDialog";
import {
  KIND_LABELS,
  STATE_LABELS,
  headingOf,
  listSummary,
  rowSubLine,
  type ListSummary,
} from "./present";
import { KIND_ICONS, StatePill } from "./RequestPills";

interface RequestsScreenProps {
  slug: string;
  search: ListSearch;
  /** The signed-in human; null before a name is given. */
  human: string | null;
  onSearch: (next: ListSearch) => void;
}

/** Below this width the list shows one card per request instead of the table. */
const PHONE = "(max-width: 47.99rem)";
const COLUMNS = ["Request", "State", "Requester", "Drafter", "Age"];
const ALL_KINDS = "all";
const FILTER_BUTTON =
  "inline-flex items-center gap-8 rounded-s py-7 pr-10 pl-12 font-title text-14 leading-tight font-semibold whitespace-nowrap disabled:cursor-not-allowed disabled:opacity-60 max-sm:min-h-44";

function filterTone(on: boolean): string {
  return on ? "bg-accent-soft text-accent" : "bg-fill text-text hover:bg-line";
}

function Header({ onNew }: { onNew: () => void }) {
  return (
    <div className="flex flex-wrap items-end justify-between gap-x-24 gap-y-12">
      <div className="flex flex-col gap-4">
        <h1 className="font-title text-24 leading-tight font-semibold text-text md:text-28">
          Requests
        </h1>
        <p className="max-w-measure text-15 text-text-2">
          Problems, ideas and questions from people and agents. An orchestrator
          drafts the cards; the requester approves them.
        </p>
      </div>
      <Button
        icon={<Plus aria-hidden="true" size={16} />}
        className="max-sm:w-full"
        onClick={onNew}
      >
        New request
      </Button>
    </div>
  );
}

function KindMenu({
  search,
  onSearch,
}: {
  search: ListSearch;
  onSearch: (next: ListSearch) => void;
}) {
  const label = search.kind === undefined ? "all" : KIND_LABELS[search.kind];
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          className={cn(
            FILTER_BUTTON,
            filterTone(search.kind !== undefined),
            "data-[state=open]:bg-accent-soft data-[state=open]:text-accent",
            focusRing,
          )}
        >
          Kind: {label}
          <ChevronDown aria-hidden="true" size={14} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          sideOffset={6}
          className={cn(menuContent, "min-w-max")}
        >
          <DropdownMenu.RadioGroup
            value={search.kind ?? ALL_KINDS}
            onValueChange={(value) => {
              const kind = KINDS.find((k) => k === value);
              onSearch(withKind(search, kind));
            }}
          >
            {[undefined, ...KINDS].map((kind) => {
              const Icon = kind === undefined ? null : KIND_ICONS[kind];
              return (
                <DropdownMenu.RadioItem
                  key={kind ?? ALL_KINDS}
                  value={kind ?? ALL_KINDS}
                  className={menuItem}
                >
                  <span className="flex w-14 shrink-0 justify-center text-accent">
                    <DropdownMenu.ItemIndicator>
                      <Check aria-hidden="true" size={14} />
                    </DropdownMenu.ItemIndicator>
                  </span>
                  {Icon ? (
                    <Icon
                      aria-hidden="true"
                      size={14}
                      className="shrink-0 text-text-2"
                    />
                  ) : null}
                  {kind === undefined ? "All kinds" : KIND_LABELS[kind]}
                </DropdownMenu.RadioItem>
              );
            })}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

interface FiltersProps {
  slug: string;
  search: ListSearch;
  human: string | null;
  counts: StateCounts | null;
  summary: ListSummary | string;
  onSearch: (next: ListSearch) => void;
}

function Filters({
  slug,
  search,
  human,
  counts,
  summary,
  onSearch,
}: FiltersProps) {
  const states = counts ? visibleStates(counts, search.state) : [];
  return (
    <div className="flex flex-wrap items-center gap-10">
      <nav
        aria-label="Request state"
        className="flex max-w-full flex-wrap gap-2 rounded-m bg-fill p-3"
      >
        {[undefined, ...states].map((state) => {
          const current = state === search.state;
          const count = counts?.[state ?? "all"];
          return (
            <HrefLink
              key={state ?? "all"}
              href={listHref(slug, { ...search, state })}
              aria-current={current ? "page" : undefined}
              className={cn(
                "rounded-s px-12 py-6 font-title text-14 leading-tight whitespace-nowrap",
                current
                  ? "bg-surface font-semibold text-text"
                  : "font-medium text-text-2 hover:text-text",
                focusRing,
              )}
            >
              {state === undefined ? "All" : STATE_LABELS[state]}
              {count === undefined ? null : (
                <>
                  {" "}
                  <span className="tabular-nums">{count}</span>
                </>
              )}
            </HrefLink>
          );
        })}
      </nav>
      <KindMenu search={search} onSearch={onSearch} />
      <button
        type="button"
        aria-pressed={search.mine === true}
        disabled={human === null}
        onClick={() => {
          onSearch(toggleMine(search));
        }}
        className={cn(
          FILTER_BUTTON,
          filterTone(search.mine === true),
          focusRing,
        )}
      >
        <UserRound aria-hidden="true" size={14} />
        Mine
      </button>
      <p className="text-14 text-text sm:ml-auto max-sm:basis-full">
        {typeof summary === "string" ? (
          <span className="text-text-2">{summary}</span>
        ) : (
          <>
            {summary.total}
            {summary.waiting === null ? null : (
              <>
                {" · "}
                <span className="font-semibold text-warn">
                  {summary.waiting}
                </span>
              </>
            )}
          </>
        )}
      </p>
    </div>
  );
}

function SkeletonRows() {
  return (
    <tbody aria-busy="true" aria-label="Loading requests">
      {[0, 1, 2, 3, 4].map((row) => (
        <tr key={row} className={TABLE_ROW}>
          {COLUMNS.map((column) => (
            <td
              key={column}
              className={cn(TABLE_CELL, "first:pl-18 last:pr-18")}
            >
              <span className="block h-12 animate-pulse rounded-s bg-sunken motion-reduce:animate-none" />
            </td>
          ))}
        </tr>
      ))}
    </tbody>
  );
}

/** The phone's loading state: the cards the list will show, as quiet bars. */
function SkeletonCards() {
  return (
    <ul
      aria-busy="true"
      aria-label="Loading requests"
      className="flex flex-col gap-8"
    >
      {[0, 1, 2, 3, 4].map((card) => (
        <li
          key={card}
          className="flex flex-col gap-8 rounded-m bg-surface px-14 py-12"
        >
          <span className="block h-14 w-full animate-pulse rounded-s bg-sunken motion-reduce:animate-none" />
          <span className="block h-12 w-skeleton-l animate-pulse rounded-s bg-sunken motion-reduce:animate-none" />
        </li>
      ))}
    </ul>
  );
}

function Table({ children }: { children: ReactNode }) {
  return (
    <div className="overflow-x-auto rounded-m bg-surface">
      <table className="w-full border-collapse">
        <thead>
          <tr>
            {COLUMNS.map((column) => (
              <th
                key={column}
                className={cn(
                  TABLE_HEAD,
                  "whitespace-nowrap first:pl-18 last:pr-18",
                  column === "Request" && "w-full",
                )}
              >
                {column}
              </th>
            ))}
          </tr>
        </thead>
        {children}
      </table>
    </div>
  );
}

function TitleLink({
  slug,
  request,
  className,
}: {
  slug: string;
  request: RequestSummary;
  className?: string;
}) {
  const heading = headingOf(request);
  return (
    <HrefLink
      href={projectPath(slug, "requests", request.id)}
      className={cn(
        "rounded-s text-15 leading-tight text-text hover:underline",
        heading.excerpt ? "font-regular" : "font-semibold",
        focusRing,
        className,
      )}
    >
      {heading.text}
    </HrefLink>
  );
}

/** The table body; the screen keeps one table from loading to loaded. */
function RequestRows({
  slug,
  rows,
  human,
  now,
}: {
  slug: string;
  rows: readonly RequestSummary[];
  human: string | null;
  now: number;
}) {
  const cell = cn(TABLE_CELL, "align-middle first:pl-18 last:pr-18");
  return (
    <tbody>
      {rows.map((r) => (
        <tr key={r.id} className={TABLE_LINK_ROW}>
          <td className={cn(cell, "max-w-0")}>
            <TitleLink slug={slug} request={r} className="block truncate" />
            <span className="mt-2 block truncate text-13 text-text-2 tabular-nums">
              {rowSubLine(r)}
            </span>
          </td>
          <td className={cn(cell, "whitespace-nowrap")}>
            <StatePill request={r} human={human} />
          </td>
          <td className={cn(cell, "whitespace-nowrap")}>
            <PersonName name={r.requester} slot="requester" />
          </td>
          <td className={cn(cell, "whitespace-nowrap")}>
            {r.drafter === null ? (
              <span className="text-text-2">Not claimed</span>
            ) : (
              <PersonName name={r.drafter} slot="requester" />
            )}
          </td>
          <td
            className={cn(
              cell,
              "text-13 whitespace-nowrap text-text-2 tabular-nums",
            )}
          >
            {formatAge(now, r.created_at)}
          </td>
        </tr>
      ))}
    </tbody>
  );
}

function RequestCards({
  slug,
  rows,
  human,
  now,
}: {
  slug: string;
  rows: readonly RequestSummary[];
  human: string | null;
  now: number;
}) {
  return (
    <ul aria-label="Requests" className="flex flex-col gap-8">
      {rows.map((r) => (
        <li
          key={r.id}
          className="relative flex flex-col gap-6 rounded-m bg-surface px-14 py-12 hover:bg-fill/45"
        >
          {/* The title's link covers the whole card, so a tap anywhere on it opens the request while its name stays the title. */}
          <TitleLink
            slug={slug}
            request={r}
            className="line-clamp-2 leading-normal after:absolute after:inset-0 after:rounded-m"
          />
          <p className="flex flex-wrap items-center gap-x-10 gap-y-6">
            <StatePill request={r} human={human} />
            <span className="text-13 text-text-2 tabular-nums">
              {rowSubLine(r, formatAge(now, r.created_at))}
            </span>
          </p>
        </li>
      ))}
    </ul>
  );
}

export function RequestsScreen({
  slug,
  search,
  human,
  onSearch,
}: RequestsScreenProps) {
  const list = useQuery(requestsQuery(slug));
  const now = useNow(true, 30_000);
  const phone = useMediaQuery(PHONE);
  const [creating, setCreating] = useState(false);
  const data = list.data;
  const rows = data ? filterRequests(data, search, human) : [];
  const summary = data
    ? listSummary(data)
    : list.isError
      ? "Not loaded"
      : "Loading requests";

  return (
    <div className="flex flex-col gap-16 px-16 pt-16 pb-24 md:gap-18 md:px-28 md:pt-24 md:pb-32">
      <Header
        onNew={() => {
          setCreating(true);
        }}
      />
      <Filters
        slug={slug}
        search={search}
        human={human}
        counts={data ? countByState(data, search, human) : null}
        summary={summary}
        onSearch={onSearch}
      />
      {list.isError ? (
        <FailurePanel
          title="Could not load the requests"
          text="The daemon answered with an error. Nothing is lost; try again."
          code={describeFailure(list.error).code}
          action={
            <Button
              icon={<RefreshCw aria-hidden="true" size={16} />}
              onClick={() => void list.refetch()}
            >
              Try again
            </Button>
          }
        />
      ) : data === undefined ? (
        phone ? (
          <SkeletonCards />
        ) : (
          <Table>
            <SkeletonRows />
          </Table>
        )
      ) : rows.length === 0 ? (
        <section
          aria-label={
            data.length === 0 ? "No requests yet" : "No request matches"
          }
          className="flex flex-col items-center gap-10 rounded-m bg-surface px-24 py-40 text-center"
        >
          <Inbox aria-hidden="true" size={24} className="text-text-2" />
          <h2 className="font-title text-18 leading-tight font-semibold text-text">
            {data.length === 0 ? "No requests yet" : "No request matches"}
          </h2>
          <p className="max-w-measure text-15 text-text-2">
            {data.length === 0
              ? "Anyone can report a problem, ask for a feature or ask a question. An orchestrator drafts the cards, and the requester approves them."
              : isNarrowed(search)
                ? "Change or clear the filters to see more requests."
                : "Pick another state to see more requests."}
          </p>
          {data.length === 0 ? (
            <>
              <Button
                variant="primary"
                icon={<Plus aria-hidden="true" size={16} />}
                onClick={() => {
                  setCreating(true);
                }}
              >
                New request
              </Button>
              <p className="text-13 text-text-2">
                An agent creates one with the create_request tool.
              </p>
            </>
          ) : null}
        </section>
      ) : phone ? (
        <RequestCards slug={slug} rows={rows} human={human} now={now} />
      ) : (
        <Table>
          <RequestRows slug={slug} rows={rows} human={human} now={now} />
        </Table>
      )}
      <NewRequestDialog
        slug={slug}
        human={human}
        open={creating}
        onOpenChange={setCreating}
      />
    </div>
  );
}
