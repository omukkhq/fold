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
        "inline-flex items-center gap-6 rounded-s px-8 py-2 text-meta leading-normal font-medium whitespace-nowrap",
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

export function StepText({ step }: { step: Step }) {
  return (
    <span className="flex max-w-(--width-search) min-w-0 items-center gap-6 text-code">
      {step.tool ? (
        <span className="font-medium text-text">{step.tool}</span>
      ) : null}
      {step.text ? (
        <span
          className={cn(
            "truncate",
            step.faint ? "text-text-faint" : "text-text-muted",
          )}
        >
          {step.text}
        </span>
      ) : null}
    </span>
  );
}

/** A worker's last error under its step: what it means, Claude Code's text cut to the cell with the whole text as its tooltip, and the hint. */
export function RowErrorText({ error }: { error: RowError }) {
  return (
    <span className="mt-2 flex w-search min-w-0 flex-col gap-2 text-meta">
      <span className="flex items-center gap-4 font-medium text-danger">
        <CircleAlert aria-hidden="true" size={12} className="shrink-0" />
        {error.title}
      </span>
      <span className="truncate font-mono text-text-muted" title={error.text}>
        {error.text}
      </span>
      {error.hint === null ? null : (
        <span className="text-text-muted">
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
        "flex items-start gap-10 rounded-m bg-danger-soft px-12 py-10",
        className,
      )}
    >
      <CircleAlert
        aria-hidden="true"
        size={15}
        className="mt-2 shrink-0 text-danger"
      />
      <p className="flex min-w-0 flex-col gap-2 text-bar text-danger">
        <span>
          <CodeText text={failure.text} />
        </span>
        {failure.code ? (
          <span className="font-mono text-meta text-text-muted">
            code: {failure.code}
          </span>
        ) : null}
      </p>
    </div>
  );
}
