import type { ComponentProps, ReactNode } from "react";
import { cn } from "@/lib/cn";
import {
  diffLineStyle,
  diffNumberClass,
  diffRowClass,
  type DiffLineKind,
} from "./diffLineModel";

interface DiffLineProps extends Omit<ComponentProps<"div">, "children"> {
  kind: DiffLineKind;
  /** The line number in the old file; absent on an added line. */
  oldNo?: number;
  /** The line number in the new file; absent on a removed line. */
  newNo?: number;
  selected?: boolean;
  /** The line text. */
  children: ReactNode;
  /** The comment marker or any other mark at the end of the row. */
  marker?: ReactNode;
}

const NUMBER_CLASSES =
  "w-diff-number shrink-0 select-none text-right font-mono text-label";

export function DiffLine({
  kind,
  oldNo,
  newNo,
  selected = false,
  children,
  marker,
  className,
  ...rest
}: DiffLineProps) {
  const style = diffLineStyle(kind);
  return (
    <div
      {...rest}
      data-kind={kind}
      data-selected={selected ? "" : undefined}
      className={cn(
        "flex items-center",
        diffRowClass(kind, selected),
        className,
      )}
    >
      <span className="sr-only">{style.description}</span>
      {style.header ? (
        <code className="min-w-0 flex-1 px-14 py-4 font-mono text-label leading-snug whitespace-pre-wrap wrap-anywhere text-text-muted">
          {children}
        </code>
      ) : (
        <>
          <span
            aria-hidden="true"
            className={cn(NUMBER_CLASSES, diffNumberClass(selected))}
          >
            {oldNo}
          </span>
          <span
            aria-hidden="true"
            className={cn(NUMBER_CLASSES, diffNumberClass(selected))}
          >
            {newNo}
          </span>
          <span
            aria-hidden="true"
            className={cn(
              "w-24 shrink-0 select-none text-center font-mono text-code",
              style.signClass,
            )}
          >
            {style.sign}
          </span>
          <code className="min-w-0 flex-1 font-mono text-code leading-loose whitespace-pre-wrap wrap-anywhere text-text">
            {children}
          </code>
        </>
      )}
      {marker ? <span className="shrink-0 pr-8">{marker}</span> : null}
    </div>
  );
}
