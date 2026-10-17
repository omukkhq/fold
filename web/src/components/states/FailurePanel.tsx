import { CircleAlert } from "lucide-react";
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
  /** The daemon's next step, which names the paths of the case. */
  hint?: string | null;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <div
      role="alert"
      className={cn(
        "flex items-start gap-10 rounded-m bg-danger-soft px-14 py-12 inset-ring inset-ring-danger",
        className,
      )}
    >
      <CircleAlert
        aria-hidden="true"
        size={15}
        className="mt-2 shrink-0 text-danger"
      />
      <div className="flex min-w-0 flex-1 flex-col gap-4">
        <p className="text-body leading-snug font-semibold text-text">
          {title}
        </p>
        <p className="text-bar leading-snug text-text-muted">{text}</p>
        {hint ? (
          <p className="text-bar leading-snug break-words text-text">{hint}</p>
        ) : null}
        {code ? (
          <p className="font-mono text-meta text-text-muted">code: {code}</p>
        ) : null}
      </div>
      {action}
    </div>
  );
}
