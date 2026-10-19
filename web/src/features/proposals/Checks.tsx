import { useQuery } from "@tanstack/react-query";
import {
  CircleCheck,
  CircleDashed,
  CircleMinus,
  CircleX,
  Clock,
  LoaderCircle,
  type LucideIcon,
} from "lucide-react";
import { useState } from "react";
import type { CheckOutcome, CheckRun, CheckRunState } from "@/api/proposals";
import { checkRunQuery } from "@/api/queries";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { logTail, resultOf, runViews, type CheckLine } from "./checkRuns";

const OVERLINE =
  "text-overline leading-snug font-semibold text-primary-text uppercase";

const STATE_ICONS: Record<
  CheckRunState,
  { icon: LucideIcon; label: string; className: string }
> = {
  running: {
    icon: LoaderCircle,
    label: "running",
    className: "text-text-muted",
  },
  passed: {
    icon: CircleCheck,
    label: "passed",
    className: "text-primary-text",
  },
  failed: { icon: CircleX, label: "failed", className: "text-danger" },
  stopped: {
    icon: CircleMinus,
    label: "stopped",
    className: "text-text-muted",
  },
};

const OUTCOME_ICONS: Record<
  CheckOutcome,
  { icon: LucideIcon; className: string }
> = {
  passed: { icon: CircleCheck, className: "text-primary-text" },
  failed: { icon: CircleX, className: "text-danger" },
  timed_out: { icon: Clock, className: "text-danger" },
  skipped: { icon: CircleDashed, className: "text-text-muted" },
  not_run: { icon: CircleMinus, className: "text-text-muted" },
};

/** The stored end of one command's output, read with the run's logs when the human opens it. */
function LogTail({
  slug,
  run,
  name,
}: {
  slug: string;
  run: string;
  name: string;
}) {
  const query = useQuery(checkRunQuery(slug, run));
  if (query.isPending) {
    return <p className="text-meta text-text-muted">Reading the log…</p>;
  }
  if (query.isError) {
    return (
      <p role="alert" className="text-meta font-medium text-danger">
        The log could not be read.
      </p>
    );
  }
  const result = resultOf(query.data, name);
  if (!result) {
    return <p className="text-meta text-text-muted">No log is stored.</p>;
  }
  return (
    <pre className="overflow-x-auto rounded-m bg-sunken px-12 py-10 font-mono text-caption whitespace-pre text-text">
      {logTail(result)}
    </pre>
  );
}

function Line({
  slug,
  run,
  line,
}: {
  slug: string;
  run: string;
  line: CheckLine;
}) {
  const [open, setOpen] = useState(line.outcome === "failed");
  const spec = OUTCOME_ICONS[line.outcome];
  const Icon = spec.icon;
  return (
    <li data-outcome={line.outcome} className="flex flex-col gap-6">
      <div className="flex gap-8">
        <Icon
          aria-hidden="true"
          size={15}
          className={cn("mt-2 shrink-0", spec.className)}
        />
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <p className="font-mono text-code font-semibold break-all text-text">
            {line.name}
          </p>
          <p className="text-meta text-text-muted">{line.text}</p>
        </div>
        {line.hasLog ? (
          <button
            type="button"
            aria-expanded={open}
            className={cn(
              "self-start text-meta font-semibold text-primary-text hover:underline",
              focusRing,
            )}
            onClick={() => {
              setOpen(!open);
            }}
          >
            {open ? "Hide log" : "Show log"}
          </button>
        ) : null}
      </div>
      {open && line.hasLog ? (
        <LogTail slug={slug} run={run} name={line.name} />
      ) : null}
    </li>
  );
}

/** The check runs of the shown version: what fold ran on exactly its files, and on its merge with main. */
export function ChecksSection({
  slug,
  runs,
}: {
  slug: string;
  runs: readonly CheckRun[];
}) {
  const views = runViews(runs);
  return (
    <section aria-label="Checks" className="flex flex-col gap-10">
      <h2 className={OVERLINE}>Checks</h2>
      {views.length === 0 ? (
        <p className="text-bar text-text-muted">
          fold ran no check on this version.
        </p>
      ) : (
        <ul className="flex flex-col gap-14">
          {views.map((view) => {
            const spec = STATE_ICONS[view.state];
            const Icon = spec.icon;
            return (
              <li
                key={view.id}
                data-state={view.state}
                className="flex flex-col gap-8"
              >
                <p className="flex items-center gap-8 text-bar leading-snug font-semibold text-text">
                  <Icon
                    role="img"
                    aria-label={spec.label}
                    size={15}
                    className={cn("shrink-0", spec.className)}
                  />
                  {view.title}
                  <span className="font-mono text-caption font-normal text-text-muted">
                    {view.id}
                  </span>
                </p>
                {view.note ? (
                  <p className="text-meta text-text-muted">{view.note}</p>
                ) : null}
                <ul className="flex flex-col gap-8 pl-22">
                  {view.lines.map((line) => (
                    <Line
                      key={line.name}
                      slug={slug}
                      run={view.id}
                      line={line}
                    />
                  ))}
                </ul>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
