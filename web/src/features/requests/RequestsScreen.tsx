import { TABLE_LINK_ROW, TABLE_ROW } from "@/components/table/tableStyles";
import { PersonName } from "@/people/PersonName";
import { useQuery } from "@tanstack/react-query";
import { Inbox, Plus, RefreshCw, UserRound } from "lucide-react";
import { useState, type ReactNode } from "react";
import { requestsQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { formatAge } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { useNow } from "@/live/useNow";
import { describeFailure } from "./errors";
import { FailurePanel } from "@/components/states/FailurePanel";
import {
  KINDS,
  STATES,
  countByState,
  filterRequests,
  isNarrowed,
  listHref,
  toggleKind,
  toggleMine,
  type ListSearch,
} from "./filters";
import { NewRequestDialog } from "./NewRequestDialog";
import {
  KIND_SPECS,
  STATE_SPECS,
  headingOf,
  kindOf,
  listSummary,
} from "./present";
import { KIND_ICONS, KindBadge, StatePill } from "./RequestPills";
import { projectPath } from "@/routes/paths";

interface RequestsScreenProps {
  slug: string;
  search: ListSearch;
  /** The signed-in human; null before a name is given. */
  human: string | null;
  onSearch: (next: ListSearch) => void;
}

const HEAD =
  "px-6 py-9 text-left font-title text-13 leading-tight font-semibold whitespace-nowrap text-text-2 first:pl-16 last:pr-16";
const CELL = "px-6 py-12 align-middle first:pl-16 last:pr-16";
const MONO = "font-mono text-13 whitespace-nowrap text-text-2";
const COLUMNS = ["Id", "Kind", "Title", "Requester", "State", "Drafter", "Age"];

function Header({ onNew }: { onNew: () => void }) {
  return (
    <div className="flex flex-wrap items-end justify-between gap-12">
      <div className="flex flex-col gap-4">
        <h1 className="text-24 leading-tight font-semibold text-text">
          Requests
        </h1>
        <p className="text-15 text-text-2">
          Problems, ideas and questions from people and agents. An orchestrator
          drafts the cards; the requester approves them.
        </p>
      </div>
      <Button
        variant="primary"
        icon={<Plus aria-hidden="true" size={15} />}
        onClick={onNew}
      >
        New request
      </Button>
    </div>
  );
}

interface FiltersProps {
  slug: string;
  search: ListSearch;
  human: string | null;
  counts: Record<string, number> | null;
  summary: string;
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
  const segments = [undefined, ...STATES];
  return (
    <div className="flex flex-wrap items-center gap-8">
      <nav
        aria-label="Request state"
        className="flex flex-wrap gap-2 rounded-m bg-sunken p-3"
      >
        {segments.map((state) => {
          const current = state === search.state;
          const label = state === undefined ? "All" : STATE_SPECS[state].label;
          const count = counts
            ? ` · ${String(counts[state ?? "all"] ?? 0)}`
            : "";
          return (
            <HrefLink
              key={state ?? "all"}
              href={listHref(slug, { ...search, state })}
              aria-current={current ? "page" : undefined}
              className={cn(
                "rounded-s px-12 py-5 text-14 leading-tight",
                current
                  ? "bg-surface font-semibold text-text inset-ring inset-ring-line"
                  : "font-medium text-text-2 hover:text-text",
                focusRing,
              )}
            >
              {label}
              {count}
            </HrefLink>
          );
        })}
      </nav>
      {KINDS.map((kind) => {
        const Icon = KIND_ICONS[kind];
        return (
          <Button
            key={kind}
            aria-pressed={search.kind === kind}
            icon={<Icon aria-hidden="true" size={15} />}
            className={cn(search.kind === kind && "bg-sunken font-semibold")}
            onClick={() => {
              onSearch(toggleKind(search, kind));
            }}
          >
            {KIND_SPECS[kind].label}
          </Button>
        );
      })}
      <Button
        aria-pressed={search.mine === true}
        disabled={human === null}
        icon={<UserRound aria-hidden="true" size={15} />}
        className={cn(search.mine && "bg-sunken font-semibold")}
        onClick={() => {
          onSearch(toggleMine(search));
        }}
      >
        Mine
      </Button>
      <p className="ml-auto text-14 text-text-2">{summary}</p>
    </div>
  );
}

function SkeletonRows() {
  return (
    <tbody aria-busy="true" aria-label="Loading requests">
      {[0, 1, 2, 3, 4].map((row) => (
        <tr key={row} className={TABLE_ROW}>
          {COLUMNS.map((column) => (
            <td key={column} className={CELL}>
              <span className="block h-12 animate-pulse rounded-s bg-sunken motion-reduce:animate-none" />
            </td>
          ))}
        </tr>
      ))}
    </tbody>
  );
}

function Table({ children }: { children: ReactNode }) {
  return (
    <div className="overflow-x-auto rounded-l border border-line bg-surface">
      <table className="w-full border-collapse">
        <thead className="bg-bg">
          <tr>
            {COLUMNS.map((column) => (
              <th
                key={column}
                className={cn(HEAD, column === "Title" && "w-full")}
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

export function RequestsScreen({
  slug,
  search,
  human,
  onSearch,
}: RequestsScreenProps) {
  const list = useQuery(requestsQuery(slug));
  const now = useNow(true, 30_000);
  const [creating, setCreating] = useState(false);
  const data = list.data;
  const rows = data ? filterRequests(data, search, human) : [];
  const summary = data
    ? listSummary(data)
    : list.isError
      ? "Not loaded"
      : "Loading requests";

  return (
    <div className="flex flex-col gap-16 px-24 pt-20 pb-24">
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
              icon={<RefreshCw aria-hidden="true" size={15} />}
              onClick={() => void list.refetch()}
            >
              Try again
            </Button>
          }
        />
      ) : data === undefined ? (
        <Table>
          <SkeletonRows />
        </Table>
      ) : rows.length === 0 ? (
        <section
          aria-label={
            data.length === 0 ? "No requests yet" : "No request matches"
          }
          className="flex flex-col items-center gap-10 rounded-l px-24 py-40 text-center inset-ring inset-ring-line"
        >
          <Inbox aria-hidden="true" size={24} className="text-text-2" />
          <h2 className="text-16 leading-tight font-semibold text-text">
            {data.length === 0 ? "No requests yet" : "No request matches"}
          </h2>
          <p className="w-thread-list max-w-full text-15 text-text-2">
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
                icon={<Plus aria-hidden="true" size={15} />}
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
      ) : (
        <Table>
          <tbody>
            {rows.map((r) => {
              const heading = headingOf(r);
              return (
                <tr key={r.id} className={TABLE_LINK_ROW}>
                  <td className={cn(CELL, MONO)}>{r.id}</td>
                  <td className={CELL}>
                    <KindBadge kind={kindOf(r)} />
                  </td>
                  <td className={cn(CELL, "max-w-0")}>
                    <HrefLink
                      href={projectPath(slug, "requests", r.id)}
                      className={cn(
                        "block truncate rounded-s text-15 leading-tight hover:underline",
                        heading.muted ? "text-text-2" : "font-medium text-text",
                        focusRing,
                      )}
                    >
                      {heading.text}
                    </HrefLink>
                  </td>
                  <td className={cn(CELL, MONO)}>
                    <PersonName name={r.requester} slot="requester" />
                  </td>
                  <td className={CELL}>
                    <StatePill state={r.state} />
                  </td>
                  <td className={cn(CELL, MONO)}>
                    {r.drafter === null ? (
                      "unclaimed"
                    ) : (
                      <PersonName name={r.drafter} slot="requester" />
                    )}
                  </td>
                  <td
                    className={cn(
                      CELL,
                      "text-13 whitespace-nowrap text-text-2",
                    )}
                  >
                    {formatAge(now, r.created_at)}
                  </td>
                </tr>
              );
            })}
          </tbody>
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
