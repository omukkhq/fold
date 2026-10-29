import { TABLE_ROW } from "@/components/table/tableStyles";
import { Id } from "@/components/id/Id";
import * as RadixTooltip from "@radix-ui/react-tooltip";
import { useQuery } from "@tanstack/react-query";
import { CircleDollarSign, Info, RefreshCw } from "lucide-react";
import { useState } from "react";
import { projectCostQuery, type ProjectCost } from "@/api/cost";
import { apiErrorCode } from "@/api/errors";
import { workersQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { Tooltip } from "@/components/overlay/Tooltip";
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
import { PersonName } from "@/people/PersonName";
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
  tileHoverText,
  type TokenPart,
  type TokensCard,
  tokensCard,
  tokensText,
  workerRows,
} from "./cost";
import { mainAgentNote } from "./note";

/** The tasks the table shows before the viewer asks for all of them. */
const TOP_TASKS = 8;

const PANEL = "flex min-w-0 flex-col gap-12 rounded-m bg-surface p-16";

const PANEL_TITLE = "text-16 leading-tight font-semibold text-text";

export function CostTab({ slug, range }: { slug: string; range: CostRange }) {
  // The range starts at the viewer's midnight, which the display clock moves past once a day.
  const now = useNow(true, 60_000);
  const query = useQuery(projectCostQuery(slug, range, rangeSince(range, now)));
  const workers = useQuery(workersQuery(slug));
  const note = mainAgentNote(workers.data?.main_supervised);
  if (query.isError) {
    return (
      <FailurePanel
        title="Could not load the cost"
        text="The daemon answered with an error. Nothing is lost; try again."
        code={apiErrorCode(query.error)}
        action={
          <Button
            icon={<RefreshCw aria-hidden="true" size={16} />}
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
      {note === null ? null : (
        <p className="flex max-w-measure items-start gap-8 text-14 text-text-2">
          <Info aria-hidden="true" size={16} className="mt-2 shrink-0" />
          {note}
        </p>
      )}
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
        className="grid grid-cols-2 gap-14 lg:grid-cols-4"
      >
        {costTiles(view).map((tile) => (
          <Tooltip key={tile.label} content={tileHoverText(tile)}>
            <li
              // The tile's tokens are its hover text, so a keyboard reaches them too.
              tabIndex={0}
              className={cn(PANEL, "gap-2 px-16 py-14", focusRing)}
            >
              <span className="font-title text-15 leading-tight font-semibold text-text">
                {tile.label}
              </span>
              <span className="font-title text-28 leading-tight font-semibold text-text tabular-nums">
                {tile.amount}
              </span>
              <span className="text-13 text-text-2 tabular-nums">
                {tile.detail}
              </span>
            </li>
          </Tooltip>
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
        <div className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-10 gap-y-8">
          <div aria-hidden="true" className="relative h-chart">
            <span className="invisible block text-13 whitespace-nowrap tabular-nums">
              {chart.axis.ticks.at(-1)?.label}
            </span>
            {chart.axis.ticks.map((tick) => (
              <span
                key={tick.at}
                className="absolute right-0 translate-y-1/2 text-13 leading-tight whitespace-nowrap text-text-2 tabular-nums"
                style={{ bottom: `${String(tick.at * 100)}%` }}
              >
                {tick.label}
              </span>
            ))}
          </div>
          <div className="relative flex h-chart items-end gap-2 px-4">
            {chart.axis.ticks.map((tick) => (
              <span
                key={tick.at}
                aria-hidden="true"
                className={cn(
                  "absolute inset-x-0 border-t",
                  tick.at === 0 ? "border-text-2/45" : "border-line",
                )}
                style={{ bottom: `${String(tick.at * 100)}%` }}
              />
            ))}
            {chart.bars.map((bar) => (
              <ChartBar key={bar.start} bar={bar} />
            ))}
          </div>
          <div className="col-start-2 flex gap-2 px-4" aria-hidden="true">
            {chart.bars.map((bar, index) => (
              <span
                key={bar.start}
                className="flex min-w-0 flex-1 justify-center text-13 whitespace-nowrap text-text-2 tabular-nums"
              >
                {showsTick(index, chart.bars.length) ? (
                  <>
                    <span className="sm:hidden">{bar.narrowTick}</span>
                    <span className="max-sm:hidden">{bar.tick}</span>
                  </>
                ) : null}
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
            "relative flex h-full min-w-0 flex-1 items-end rounded-t-s hover:bg-fill/60",
            focusRing,
          )}
        >
          <span
            className="w-full rounded-t-s bg-accent"
            style={{ height: `${String(bar.height * 100)}%` }}
          />
        </button>
      </RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <RadixTooltip.Content
          sideOffset={4}
          className="z-tooltip flex flex-col gap-2 rounded-s bg-ink px-8 py-6 text-13 text-on-ink"
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
  cache_read: "bg-accent",
  cache_write: "bg-accent/45",
  output: "bg-text-2",
  input: "bg-text-2/40",
};

function TokensPanel({ view }: { view: ProjectCost }) {
  const card = tokensCard(view);
  return (
    <section aria-label="Tokens" className={PANEL}>
      <div className="flex flex-wrap items-baseline justify-between gap-12">
        <h2 className={PANEL_TITLE}>Tokens</h2>
        <span className="font-title text-24 leading-tight font-semibold text-text tabular-nums">
          {card.total}
        </span>
      </div>
      {card.cacheLine === null ? (
        <p className="text-14 text-text-2">
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
        className="flex h-10 gap-2 overflow-hidden rounded-pill bg-fill"
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
            <dt className="flex items-center gap-6 text-13 text-text-2">
              <span
                aria-hidden="true"
                className={cn("size-9 rounded-full", TOKEN_FILL[part.key])}
              />
              {part.label}
            </dt>
            <dd className="text-15 font-semibold text-text tabular-nums">
              {part.amount}
            </dd>
          </div>
        ))}
      </dl>
      <p className="text-13 text-text-2">{card.cacheLine}</p>
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
            <span className="flex items-baseline gap-8 text-14">
              <span className="min-w-0 flex-1 truncate font-medium text-text">
                {row.name}
              </span>
              <span className="text-13 text-text-2">{row.count}</span>
              <span className="font-semibold text-text tabular-nums">
                {row.amount}
              </span>
            </span>
            <span className="h-6 rounded-pill bg-fill">
              <span
                className="block h-6 rounded-pill bg-accent"
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
  "px-12 py-8 text-left font-title text-13 leading-tight font-semibold text-text-2";
const TD = "px-12 py-9 text-14 leading-tight whitespace-nowrap text-text";
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
              "rounded-s text-14 font-medium text-accent hover:underline",
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
        <p className="px-16 text-14 text-text-2">
          No turn in this range worked on a task.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse">
            <thead>
              <tr>
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
                <tr key={row.task} className={TABLE_ROW}>
                  <td className={cn(TD, "min-w-0")}>
                    <HrefLink
                      href={taskPath(slug, row.task)}
                      className={cn(
                        "flex items-baseline gap-8 rounded-s",
                        focusRing,
                      )}
                    >
                      <Id className="shrink-0 text-accent">{row.task}</Id>
                      <span className="truncate hover:underline">
                        {row.title}
                      </span>
                    </HrefLink>
                  </td>
                  <td className={cn(TD, "text-text-2")}>{row.area}</td>
                  <td className={cn(TD, NUM)}>{row.build}</td>
                  <td className={cn(TD, NUM, "text-text-2")}>{row.review}</td>
                  <td className={cn(TD, NUM, "font-semibold")}>{row.total}</td>
                  <td className={cn(TD, NUM, "text-text-2")}>{row.tokens}</td>
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
        <p className="text-13 text-text-2">
          Stopped, restarted and removed workers stay counted.
        </p>
      </div>
      <div className="overflow-x-auto">
        <table className="w-full border-collapse">
          <thead>
            <tr>
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
              <tr key={row.name} className={TABLE_ROW}>
                <td className={TD}>
                  <span className="flex items-baseline gap-8">
                    <PersonName name={row.name} />
                    {row.ended ? (
                      <span className="text-13 text-text-2">stopped</span>
                    ) : null}
                  </span>
                </td>
                <td className={cn(TD, "text-text-2")}>{row.role}</td>
                <td className={cn(TD, "text-text-2")}>{row.area}</td>
                <td className={cn(TD, "text-text-2")}>{row.model}</td>
                <td className={cn(TD, NUM)}>{row.turns}</td>
                <td className={cn(TD, NUM, "text-text-2")}>{row.tokens}</td>
                <td className={cn(TD, NUM, "text-text-2")}>{row.output}</td>
                <td className={cn(TD, NUM, "font-semibold")}>{row.cost}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
