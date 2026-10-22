import { useQuery } from "@tanstack/react-query";
import { Copy, ListChecks } from "lucide-react";
import { useState } from "react";
import { checkRunQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { copyOutcome, type CopyState } from "@/components/mono/commitCopy";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { Skeleton } from "@/components/states/Skeleton";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { outcomeText, reasonText, resultOf } from "./checkRuns";
import {
  chosenStep,
  firstError,
  logLines,
  runTitle,
  stepMeta,
  type RunItem,
} from "./runList";
import { OUTCOME_ICONS, RUN_ICONS } from "./checkIcons";
import { detailHref, type DetailSearch } from "./view";

function CopyLog({ log }: { log: string }) {
  const [state, setState] = useState<CopyState>("idle");
  return (
    <span className="inline-flex items-center gap-8">
      <Button
        size="sm"
        icon={<Copy aria-hidden="true" size={13} />}
        onClick={() => {
          void copyOutcome(log, navigator.clipboard).then(setState);
        }}
      >
        Copy log
      </Button>
      {state === "idle" ? null : (
        <span role="status" className="text-meta text-text-muted">
          {state === "copied" ? "Copied." : "The browser refused the copy."}
        </span>
      )}
    </span>
  );
}

function LogPanel({
  slug,
  item,
  step,
}: {
  slug: string;
  item: RunItem;
  step: string;
}) {
  const query = useQuery(checkRunQuery(slug, item.run.id));
  const listed = item.run.results.find((r) => r.name === step);
  const spec = OUTCOME_ICONS[listed?.outcome ?? "not_run"];
  const Icon = spec.icon;
  const result = resultOf(query.data, step);
  const lines = result ? logLines(result.log) : [];
  const error = firstError(lines);
  return (
    <section
      aria-label={`Log of ${step}`}
      className="flex min-w-0 flex-1 flex-col gap-12"
    >
      <div className="flex flex-wrap items-center gap-x-12 gap-y-6">
        <Icon
          aria-hidden="true"
          size={16}
          className={cn("shrink-0", spec.className)}
        />
        <h2 className="font-mono text-section font-medium text-text">{step}</h2>
        <span className="text-code text-text-muted">
          {listed ? outcomeText(listed) : ""} · run{" "}
          <span className="font-mono">{item.run.id}</span> on v{item.version},
          base {item.run.base}
        </span>
        <span className="flex flex-wrap items-center gap-8 md:ml-auto">
          {error === null ? null : (
            <Button
              size="sm"
              onClick={() => {
                document
                  .getElementById(`log-line-${String(error)}`)
                  ?.scrollIntoView({ block: "center" });
              }}
            >
              Jump to first error
            </Button>
          )}
          {result && result.log !== "" ? <CopyLog log={result.log} /> : null}
        </span>
      </div>
      {query.isPending ? (
        <Skeleton />
      ) : query.isError ? (
        <p role="alert" className="text-meta font-medium text-danger">
          The log could not be read.
        </p>
      ) : !result || lines.length === 0 ? (
        <p className="text-code text-text-muted">No log is stored.</p>
      ) : (
        <div className="overflow-x-auto rounded-l bg-surface py-10 inset-ring inset-ring-border-subtle">
          {result.truncated ? (
            <p className="px-14 pb-6 text-meta text-text-faint">
              fold keeps only the end of a long log.
            </p>
          ) : null}
          <ol className="font-mono text-code leading-snug">
            {lines.map((line) => (
              <li
                key={line.n}
                id={`log-line-${String(line.n)}`}
                data-error={line.error || undefined}
                className={cn("flex", line.error && "bg-danger-soft")}
              >
                <span
                  aria-hidden="true"
                  className="w-diff-number shrink-0 pr-12 text-right text-text-faint select-none"
                >
                  {line.n}
                </span>
                <code
                  className={cn(
                    "pr-14 whitespace-pre",
                    line.error ? "text-danger" : "text-text",
                  )}
                >
                  {line.text}
                </code>
              </li>
            ))}
          </ol>
        </div>
      )}
    </section>
  );
}

export interface ChecksTabProps {
  slug: string;
  id: string;
  /** Every run of every version, newest first; null until the runs of every version arrived. */
  items: readonly RunItem[] | null;
  search: DetailSearch;
  now: number;
}

/** Each run with its steps on one side, and the log of the chosen step across the rest of the width. */
export function ChecksTab({ slug, id, items, search, now }: ChecksTabProps) {
  if (items === null) {
    return <Skeleton className="m-24" />;
  }
  if (items.length === 0) {
    return (
      <div className="px-16 py-20 md:px-24">
        <EmptyPanel icon={ListChecks} title="No check runs">
          fold ran no check on any version of this proposal.
        </EmptyPanel>
      </div>
    );
  }
  const chosen = chosenStep(items, search);
  const chosenItem = items.find((item) => item.run.id === chosen?.run);
  return (
    <div className="flex min-w-0 flex-col gap-20 px-16 py-16 md:px-24 lg:flex-row lg:items-start">
      <nav
        aria-label="Check runs"
        className="flex min-w-0 flex-col gap-18 lg:w-review-panel lg:shrink-0"
      >
        {items.map((item) => {
          const spec = RUN_ICONS[item.run.state];
          const Icon = spec.icon;
          return (
            <section
              key={item.run.id}
              aria-label={`${runTitle(item)} ${item.run.id}`}
              data-state={item.run.state}
              className="flex flex-col gap-4"
            >
              <div className="flex items-center gap-8 px-4 pb-4">
                <Icon
                  role="img"
                  aria-label={spec.label}
                  size={15}
                  className={cn("shrink-0", spec.className)}
                />
                <h3 className="text-bar font-semibold text-text">
                  {runTitle(item)}
                </h3>
                <span className="font-mono text-caption text-text-faint">
                  {item.run.id}
                </span>
                <span className="ml-auto text-meta text-text-faint">
                  {formatAsOfMinute(now, item.run.created_at)}
                </span>
              </div>
              {item.run.reason === null ? null : (
                <p className="px-4 text-meta text-text-muted">
                  {reasonText(item.run.reason)}
                </p>
              )}
              <ul className="flex flex-col">
                {item.run.results.map((result) => {
                  const step = OUTCOME_ICONS[result.outcome];
                  const StepIcon = step.icon;
                  const current =
                    chosen?.run === item.run.id && chosen.step === result.name;
                  return (
                    <li key={result.name}>
                      <HrefLink
                        href={detailHref(slug, id, {
                          tab: "checks",
                          run: item.run.id,
                          step: result.name,
                        })}
                        aria-current={current ? "true" : undefined}
                        data-outcome={result.outcome}
                        className={cn(
                          "flex items-center gap-8 rounded-s px-8 py-5 hover:bg-sunken",
                          current &&
                            (result.outcome === "failed" ||
                            result.outcome === "timed_out"
                              ? "bg-danger-soft hover:bg-danger-soft"
                              : "bg-primary-soft hover:bg-primary-soft"),
                          focusRing,
                        )}
                      >
                        <StepIcon
                          aria-hidden="true"
                          size={14}
                          className={cn("shrink-0", step.className)}
                        />
                        <span className="min-w-0 flex-1 truncate font-mono text-code text-text">
                          {result.name}
                        </span>
                        <span className="shrink-0 text-meta text-text-faint">
                          {stepMeta(result)}
                        </span>
                      </HrefLink>
                    </li>
                  );
                })}
              </ul>
            </section>
          );
        })}
        <p className="px-4 text-meta leading-normal text-text-faint">
          Newest run first. The failed step of the newest failed run opens on
          its own.
        </p>
      </nav>
      {chosen && chosenItem ? (
        <LogPanel
          key={`${chosen.run}:${chosen.step}`}
          slug={slug}
          item={chosenItem}
          step={chosen.step}
        />
      ) : null}
    </div>
  );
}
