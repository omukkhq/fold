import type { ComponentProps, ReactNode } from "react";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";

interface CardProps extends Omit<ComponentProps<"div">, "title"> {
  header?: ReactNode;
  footer?: ReactNode;
  selected?: boolean;
}

/** The board card shell. It has slots and a state, and no data logic. A card reads on the page ground by its surface alone; only the selected card has a ring. */
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
        "flex flex-col gap-8 rounded-m bg-surface px-14 py-12",
        selected && "bg-accent-soft inset-ring inset-ring-accent",
        focusRing,
        className,
      )}
    >
      {header}
      {children}
      {footer ? (
        <div className="flex items-center justify-between gap-8 pt-4">
          {footer}
        </div>
      ) : null}
    </div>
  );
}
