import * as RadixTooltip from "@radix-ui/react-tooltip";
import { useQuery } from "@tanstack/react-query";
import { CircleDollarSign, Info, RefreshCw } from "lucide-react";
import { Fragment, useState } from "react";
import { projectCostQuery, type ProjectCost } from "@/api/cost";
import { apiErrorCode } from "@/api/errors";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { Mono } from "@/components/mono/Mono";
import {
  SEGMENT_LIST,
  segmentClasses,
} from "@/components/segments/segmentStyles";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { FailurePanel } from "@/components/states/FailurePanel";
import { Skeleton } from "@/components/states/Skeleton";
import { taskPath } from "@/features/board/card";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { useNow } from "@/live/useNow";
import {
  areaRows,
  barLabel,
  type BreakdownRow,
  CHART_MEASURES,
  type ChartMeasure,
  type CostBar,
  costChart,
  costText,
  costTiles,
  type CostRange,
  isEmptyCost,
  modelRows,
  rangeSince,
  roleRowName,
  roleRows,
  showsTick,
  taskRows,
  type TokenPart,
  type TokensCard,
  tokensCard,
  tokensText,
  workerRows,
} from "./cost";

/** The tasks the table shows before the viewer asks for all of them. */
const TOP_TASKS = 8;

const PANEL =
  "flex min-w-0 flex-col gap-12 rounded-l bg-surface p-16 inset-ring inset-ring-border-subtle";

const PANEL_TITLE = "text-section leading-snug font-semibold text-text";

/** The main agent's session is outside fold's view, and fold guesses no number for it. */
export const MAIN_AGENT_NOTE =
  "The main agent runs in your own Claude Code session, which fold does not supervise, so its cost is not in these numbers.";

export function CostTab({ slug, range }: { slug: string; range: CostRange }) {
  // The range starts at the viewer's midnight, which the display clock moves past once a day.
  const now = useNow(true, 60_000);
  const query = useQuery(projectCostQuery(slug, range, rangeSince(range, now)));
  if (query.isError) {
    return (
      <FailurePanel
        title="Could not load the cost"
        text="The daemon answered with an error. Nothing is lost; try again."
        code={apiErrorCode(query.error)}
        action={
          <Button
            icon={<RefreshCw aria-hidden="true" size={15} />}
            onClick={() => void query.refetch()}
          >
            Try again
          </Button>
        }
      />
    );
  }
  if (query.data === undefined) {
    return (
      <div className="flex flex-col gap-16">
        <Skeleton />
        <Skeleton />
      </div>
    );
  }
  const view = query.data;
  return (
    <div className="flex flex-col gap-16">
      {isEmptyCost(view) ? (
        <EmptyPanel icon={CircleDollarSign} title="No cost in this range">
          fold counts the cost of each turn of the workers it runs. A range
          without a turn has nothing to show.
        </EmptyPanel>
      ) : (
        <CostContent slug={slug} view={view} range={range} now={now} />
      )}
      <p className="flex items-start gap-8 text-bar text-text-muted">
        <Info aria-hidden="true" size={15} className="mt-2 shrink-0" />
        {MAIN_AGENT_NOTE}
      </p>
    </div>
  );
}

function CostContent({
  slug,
  view,
  range,
  now,
}: {
  slug: string;
  view: ProjectCost;
  range: CostRange;
  now: number;
}) {
  return (
    <>
      <ul
        aria-label="Totals"
        className="grid grid-cols-2 gap-12 lg:grid-cols-4"
      >
        {costTiles(view).map((tile, index) => (
          <li key={tile.label} className={cn(PANEL, "gap-4")}>
            <span className="text-overline leading-snug font-semibold text-text-muted uppercase">
              {tile.label}
            </span>
            <span
              className={cn(
                "leading-snug font-semibold text-text tabular-nums",
                index === 0 ? "text-page" : "text-screen",
              )}
            >
              {tile.amount}
            </span>
            <span className="text-meta text-text-muted">{tile.detail}</span>
            <span className="text-meta text-text-muted tabular-nums">
              {tile.tokens.map((part, at) => (
                <Fragment key={part}>
                  {at > 0 ? " · " : null}
                  <span className="whitespace-nowrap">{part}</span>
                </Fragment>
              ))}
            </span>
          </li>
        ))}
      </ul>
      <Chart view={view} range={range} now={now} />
      <TokensPanel view={view} />
      <div className="grid grid-cols-1 gap-12 lg:grid-cols-3">
        <Breakdown title="By role" rows={roleRows(view)} />
        <Breakdown title="By area" rows={areaRows(view)} />
        <Breakdown title="By model" rows={modelRows(view)} />
      </div>
      <TasksTable slug={slug} view={view} />
      <WorkersTable view={view} />
    </>
  );
}

function Chart({
  view,
  range,
  now,
}: {
  view: ProjectCost;
  range: CostRange;
  now: number;
}) {
  const [measure, setMeasure] = useState<ChartMeasure>("cost");
  const chart = costChart(view, range, now, measure);
  return (
    <section aria-label={chart.title} className={PANEL}>
      <div className="flex flex-wrap items-center justify-between gap-12">
        <h2 className={PANEL_TITLE}>{chart.title}</h2>
        <div role="group" aria-label="Measure" className={SEGMENT_LIST}>
          {CHART_MEASURES.map((option) => (
            <button
              key={option.value}
              type="button"
              aria-pressed={measure === option.value}
              onClick={() => {
                setMeasure(option.value);
              }}
              className={cn(
                segmentClasses(measure === option.value),
                focusRing,
              )}
            >
              {option.label}
            </button>
          ))}
        </div>
      </div>
      <RadixTooltip.Provider delayDuration={0}>
        <div className="flex flex-col gap-6">
          <div className="flex h-chart items-end gap-2 border-b border-border-subtle">
            {chart.bars.map((bar) => (
              <ChartBar key={bar.start} bar={bar} />
            ))}
          </div>
          <div className="flex gap-2" aria-hidden="true">
            {chart.bars.map((bar, index) => (
              <span
                key={bar.start}
                className="min-w-0 flex-1 truncate text-center text-caption text-text-faint"
              >
                {showsTick(index, chart.bars.length) ? bar.tick : ""}
              </span>
            ))}
          </div>
        </div>
      </RadixTooltip.Provider>
    </section>
  );
}

/** A bar's hit target is its whole column, taller than the bar, so a short bar is easy to point at. */
function ChartBar({ bar }: { bar: CostBar }) {
  return (
    <RadixTooltip.Root>
      <RadixTooltip.Trigger asChild>
        <button
          type="button"
          aria-label={barLabel(bar)}
          className={cn(
            "flex h-full min-w-0 flex-1 items-end rounded-t-s hover:bg-sunken",
            focusRing,
          )}
        >
          <span
            className="w-full rounded-t-s bg-primary"
            style={{ height: `${String(bar.height * 100)}%` }}
          />
        </button>
      </RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <RadixTooltip.Content
          sideOffset={4}
          className="z-10 flex flex-col gap-2 rounded-s bg-ink px-8 py-6 text-meta text-on-ink"
        >
          <span className="font-semibold">
            {bar.title} · {costText(bar.cost)} · {tokensText(bar.tokens)} tokens
          </span>
          {bar.roles.map((part) => (
            <span key={part.role} className="flex justify-between gap-12">
              <span>{roleRowName(part.role)}</span>
              <span className="tabular-nums">
                {costText(part.cost)} · {tokensText(part.tokens)}
              </span>
            </span>
          ))}
        </RadixTooltip.Content>
      </RadixTooltip.Portal>
    </RadixTooltip.Root>
  );
}

const TOKEN_FILL: Record<TokenPart["key"], string> = {
  cache_read: "bg-primary",
  cache_write: "bg-primary-muted",
  output: "bg-secondary",
  input: "bg-text-faint",
};

function TokensPanel({ view }: { view: ProjectCost }) {
  const card = tokensCard(view);
  return (
    <section aria-label="Tokens" className={PANEL}>
      <div className="flex flex-wrap items-baseline justify-between gap-12">
        <h2 className={PANEL_TITLE}>Tokens</h2>
        <span className="text-screen leading-snug font-semibold text-text tabular-nums">
          {card.total}
        </span>
      </div>
      {card.cacheLine === null ? (
        <p className="text-bar text-text-muted">
          No turn in this range reported its tokens.
        </p>
      ) : (
        <TokenParts card={card} />
      )}
    </section>
  );
}

function TokenParts({ card }: { card: TokensCard }) {
  return (
    <>
      <div
        aria-hidden="true"
        className="flex h-6 gap-2 overflow-hidden rounded-full bg-sunken"
      >
        {card.parts.map((part) =>
          part.fill > 0 ? (
            <span
              key={part.key}
              className={cn("h-full", TOKEN_FILL[part.key])}
              style={{ width: `${String(part.fill * 100)}%` }}
            />
          ) : null,
        )}
      </div>
      <dl className="grid grid-cols-2 gap-12 lg:grid-cols-4">
        {card.parts.map((part) => (
          <div key={part.key} className="flex flex-col gap-2">
            <dt className="flex items-center gap-6 text-meta text-text-muted">
              <span
                aria-hidden="true"
                className={cn("size-7 rounded-full", TOKEN_FILL[part.key])}
              />
              {part.label}
            </dt>
            <dd className="text-body font-semibold text-text tabular-nums">
              {part.amount}
            </dd>
          </div>
        ))}
      </dl>
      <p className="text-meta text-text-muted">{card.cacheLine}</p>
    </>
  );
}

function Breakdown({ title, rows }: { title: string; rows: BreakdownRow[] }) {
  return (
    <section aria-label={title} className={PANEL}>
      <h2 className={PANEL_TITLE}>{title}</h2>
      <ul className="flex flex-col gap-10">
        {rows.map((row) => (
          <li key={row.key} className="flex flex-col gap-4">
            <span className="flex items-baseline gap-8 text-bar">
              <span className="min-w-0 flex-1 truncate font-medium text-text">
                {row.name}
              </span>
              <span className="text-meta text-text-muted">{row.count}</span>
              <span className="font-semibold text-text tabular-nums">
                {row.amount}
              </span>
            </span>
            <span className="h-4 rounded-full bg-sunken">
              <span
                className="block h-4 rounded-full bg-primary"
                style={{ width: `${String(row.fill * 100)}%` }}
              />
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

const TH =
  "px-12 py-8 text-left text-overline leading-snug font-semibold text-text-muted uppercase";
const TD = "px-12 py-9 text-bar leading-snug whitespace-nowrap text-text";
const NUM = "text-right tabular-nums";

function TasksTable({ slug, view }: { slug: string; view: ProjectCost }) {
  const [all, setAll] = useState(false);
  const { rows, more } = taskRows(view, all ? null : TOP_TASKS);
  return (
    <section aria-label="Tasks" className={cn(PANEL, "px-0 pb-4")}>
      <div className="flex items-center justify-between gap-12 px-16">
        <h2 className={PANEL_TITLE}>Tasks</h2>
        {more > 0 || all ? (
          <button
            type="button"
            className={cn(
              "rounded-s text-bar font-medium text-primary-text hover:underline",
              focusRing,
            )}
            onClick={() => {
              setAll(!all);
            }}
          >
            {all
              ? "Show the most expensive"
              : `Show all ${String(view.tasks.length)} tasks`}
          </button>
        ) : null}
      </div>
      {rows.length === 0 ? (
        <p className="px-16 text-bar text-text-muted">
          No turn in this range worked on a task.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse">
            <thead>
              <tr className="border-b border-border-subtle">
                <th className={TH}>Task</th>
                <th className={TH}>Area</th>
                <th className={cn(TH, NUM)}>Build</th>
                <th className={cn(TH, NUM)}>Review</th>
                <th className={cn(TH, NUM)}>Total</th>
                <th className={cn(TH, NUM)}>Tokens</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr
                  key={row.task}
                  className="border-b border-border-subtle last:border-b-0"
                >
                  <td className={cn(TD, "min-w-0")}>
                    <HrefLink
                      href={taskPath(slug, row.task)}
                      className={cn(
                        "flex items-baseline gap-8 rounded-s",
                        focusRing,
                      )}
                    >
                      <Mono className="shrink-0 text-meta text-primary-text">
                        {row.task}
                      </Mono>
                      <span className="truncate hover:underline">
                        {row.title}
                      </span>
                    </HrefLink>
                  </td>
                  <td className={cn(TD, "text-text-muted")}>{row.area}</td>
                  <td className={cn(TD, NUM)}>{row.build}</td>
                  <td className={cn(TD, NUM, "text-text-muted")}>
                    {row.review}
                  </td>
                  <td className={cn(TD, NUM, "font-semibold")}>{row.total}</td>
                  <td className={cn(TD, NUM, "text-text-muted")}>
                    {row.tokens}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function WorkersTable({ view }: { view: ProjectCost }) {
  const rows = workerRows(view);
  return (
    <section aria-label="Workers" className={cn(PANEL, "px-0 pb-4")}>
      <div className="flex flex-col gap-2 px-16">
        <h2 className={PANEL_TITLE}>Workers</h2>
        <p className="text-meta text-text-muted">
          Stopped, restarted and removed workers stay counted.
        </p>
      </div>
      <div className="overflow-x-auto">
        <table className="w-full border-collapse">
          <thead>
            <tr className="border-b border-border-subtle">
              <th className={TH}>Worker</th>
              <th className={TH}>Role</th>
              <th className={TH}>Area</th>
              <th className={TH}>Model</th>
              <th className={cn(TH, NUM)}>Turns</th>
              <th className={cn(TH, NUM)}>Tokens</th>
              <th className={cn(TH, NUM)}>Output</th>
              <th className={cn(TH, NUM)}>Cost</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr
                key={row.name}
                className="border-b border-border-subtle last:border-b-0"
              >
                <td className={TD}>
                  <span className="flex items-baseline gap-8">
                    <Mono className="text-meta text-text">{row.name}</Mono>
                    {row.ended ? (
                      <span className="text-caption text-text-faint">
                        stopped
                      </span>
                    ) : null}
                  </span>
                </td>
                <td className={cn(TD, "text-text-muted")}>{row.role}</td>
                <td className={cn(TD, "text-text-muted")}>{row.area}</td>
                <td className={cn(TD, "text-text-muted")}>
                  <Mono className="text-meta">{row.model}</Mono>
                </td>
                <td className={cn(TD, NUM)}>{row.turns}</td>
                <td className={cn(TD, NUM, "text-text-muted")}>{row.tokens}</td>
                <td className={cn(TD, NUM, "text-text-muted")}>{row.output}</td>
                <td className={cn(TD, NUM, "font-semibold")}>{row.cost}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
