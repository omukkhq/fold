import { CodeText } from "@/components/code/CodeText";
import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/** An empty list or a missing item: one surface block without an outline, and the next step. */
interface EmptyPanelProps {
  /** An empty board draws no icon, so a screen may leave it out. */
  icon?: LucideIcon;
  title: string;
  /** The next step, as text. */
  children: string;
  action?: ReactNode;
  className?: string;
}

export function EmptyPanel({
  icon: Icon,
  title,
  children,
  action,
  className,
}: EmptyPanelProps) {
  return (
    <section
      aria-label={title}
      className={cn(
        "flex items-start gap-14 rounded-m bg-surface p-24",
        className,
      )}
    >
      {Icon ? (
        <Icon
          aria-hidden="true"
          size={20}
          className="mt-2 shrink-0 text-text-2"
        />
      ) : null}
      <div className="flex min-w-0 flex-col items-start gap-8">
        <h2 className="text-18 font-semibold text-text">{title}</h2>
        <p className="max-w-measure text-15 text-text">
          <CodeText text={children} />
        </p>
        {action}
      </div>
    </section>
  );
}
