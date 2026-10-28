import { CircleAlert } from "lucide-react";
import { CodeText } from "@/components/code/CodeText";
import { cn } from "@/lib/cn";
import {
  pillSpec,
  type RowError,
  type StatePill as Pill,
  type Step,
} from "./team";
import type { Failure } from "./startLead";

export function StatePill({ pill }: { pill: Pill }) {
  const spec = pillSpec(pill.tone);
  return (
    <span
      className={cn(
        "inline-flex items-center gap-6 rounded-pill px-9 py-2 text-13 leading-tight font-medium whitespace-nowrap",
        spec.fill,
        spec.text,
      )}
    >
      <span
        aria-hidden="true"
        className={cn("size-7 rounded-full", spec.dot)}
      />
      {pill.label}
    </span>
  );
}

/** The step after the state; `warn` draws it in gold, for a worker that waits for a person's answer. */
export function StepText({
  step,
  warn = false,
}: {
  step: Step;
  warn?: boolean;
}) {
  return (
    <span
      className={cn(
        "flex max-w-full min-w-0 items-center gap-6 text-13 leading-tight",
        warn ? "text-warn" : "text-text-2",
      )}
    >
      {step.tool ? (
        <span
          className={cn("shrink-0 font-semibold", warn ? null : "text-text")}
        >
          {step.tool}
        </span>
      ) : null}
      {step.text ? (
        <span className="truncate" title={step.text}>
          {step.text}
        </span>
      ) : null}
    </span>
  );
}

/** What a worker's last error means, in the place of its step once the process has ended. */
export function ErrorTitle({ error }: { error: RowError }) {
  return (
    <span className="flex min-w-0 items-center gap-6 text-13 leading-tight font-medium text-bad">
      <CircleAlert aria-hidden="true" size={14} className="shrink-0" />
      <span className="truncate">{error.title}</span>
    </span>
  );
}

/** A worker's last error in its open row: what it means, Claude Code's text as data, and the daemon's hint. */
export function RowErrorText({ error }: { error: RowError }) {
  return (
    <span className="flex min-w-0 flex-col gap-4 text-14">
      <span className="font-medium text-bad">{error.title}</span>
      <span className="font-mono text-13 wrap-anywhere text-text">
        {error.text}
      </span>
      {error.hint === null ? null : (
        <span className="text-13 text-text-2">
          <CodeText text={error.hint} caption />
        </span>
      )}
    </span>
  );
}

/** A refused write: the sentence for its code and the code itself. */
export function ErrorNote({
  failure,
  className,
}: {
  failure: Failure;
  className?: string;
}) {
  return (
    <div
      role="alert"
      className={cn(
        "flex items-start gap-10 rounded-m bg-bad-soft px-12 py-10",
        className,
      )}
    >
      <CircleAlert
        aria-hidden="true"
        size={15}
        className="mt-2 shrink-0 text-bad"
      />
      <p className="flex min-w-0 flex-col gap-2 text-14 text-bad">
        <span>
          <CodeText text={failure.text} />
        </span>
        {failure.code ? (
          <span className="font-mono text-13 text-text-2">
            code: {failure.code}
          </span>
        ) : null}
      </p>
    </div>
  );
}
