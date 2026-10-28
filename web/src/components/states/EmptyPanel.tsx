import { CodeText } from "@/components/code/CodeText";
import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

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
        "flex flex-col items-start gap-10 rounded-m px-14 py-18 inset-ring inset-ring-line",
        className,
      )}
    >
      {Icon ? (
        <Icon aria-hidden="true" size={18} className="text-text-2" />
      ) : null}
      <h2 className="text-16 leading-tight font-semibold text-text">{title}</h2>
      <p className="text-14 leading-tight text-text-2">
        <CodeText text={children} />
      </p>
      {action}
    </section>
  );
}
