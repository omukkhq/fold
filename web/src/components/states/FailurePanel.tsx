import { CircleAlert } from "lucide-react";
import { CodeText } from "@/components/code/CodeText";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/** A failure with its sentence, the daemon's hint when it gives one, and its code, for a load or write error. */
export function FailurePanel({
  title,
  text,
  code,
  hint,
  action,
  className,
}: {
  title: string;
  text: string;
  code: string | null;
  /** The daemon's next step, which names the paths of the case and marks its commands between backticks. */
  hint?: string | null;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <div
      role="alert"
      className={cn(
        "flex items-start gap-10 rounded-m bg-bad-soft px-14 py-12 inset-ring inset-ring-bad",
        className,
      )}
    >
      <CircleAlert
        aria-hidden="true"
        size={15}
        className="mt-2 shrink-0 text-bad"
      />
      <div className="flex min-w-0 flex-1 flex-col gap-4">
        <p className="text-15 leading-tight font-semibold text-text">{title}</p>
        <p className="text-14 leading-tight text-text-2">
          <CodeText text={text} />
        </p>
        {hint ? (
          <p className="text-14 leading-tight wrap-anywhere text-text">
            <CodeText text={hint} />
          </p>
        ) : null}
        {code ? (
          <p className="font-mono text-13 text-text-2">code: {code}</p>
        ) : null}
      </div>
      {action}
    </div>
  );
}
