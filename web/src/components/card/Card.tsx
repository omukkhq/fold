import type { ComponentProps, ReactNode } from "react";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";

interface CardProps extends Omit<ComponentProps<"div">, "title"> {
  header?: ReactNode;
  footer?: ReactNode;
  selected?: boolean;
}

/** The board card shell. It has slots and a state, and no data logic. */
export function Card({
  header,
  footer,
  selected,
  className,
  children,
  ...rest
}: CardProps) {
  return (
    <div
      {...rest}
      aria-current={selected ? true : undefined}
      data-selected={selected ? "" : undefined}
      className={cn(
        "flex flex-col gap-8 rounded-m bg-surface px-12 py-10 shadow-pop inset-ring",
        selected ? "bg-accent-soft inset-ring-accent" : "inset-ring-line",
        focusRing,
        className,
      )}
    >
      {header}
      {children}
      {/* The top padding gives one pixel to the 1px border, so the border does not change the footer's height. */}
      {footer ? (
        <div className="flex items-center justify-between gap-8 border-t border-line pt-7">
          {footer}
        </div>
      ) : null}
    </div>
  );
}
