import {
  type ReactNode,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  type ClampLines,
  clampClass,
  isCut,
  showsToggle,
  toggleLabel,
} from "./clamp";
import { textFlow } from "./textFlow";

/** Long text cut to a few wrapped lines, with More and Less to open and close the rest. */
export function ClampText({
  children,
  lines = 3,
  className,
}: {
  children: ReactNode;
  lines?: ClampLines;
  className?: string;
}) {
  const id = useId();
  const box = useRef<HTMLParagraphElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [cut, setCut] = useState(false);

  useLayoutEffect(() => {
    const element = box.current;
    if (element === null || expanded) return;
    const measure = () => {
      setCut(isCut(element));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, [expanded, children]);

  return (
    <div className="flex min-w-0 flex-col items-start gap-2">
      <p
        ref={box}
        id={id}
        className={cn(
          "min-w-0 self-stretch",
          textFlow("prose"),
          clampClass(lines, expanded),
          className,
        )}
      >
        {children}
      </p>
      {showsToggle(expanded, cut) ? (
        <button
          type="button"
          aria-expanded={expanded}
          aria-controls={id}
          onClick={() => {
            setExpanded(!expanded);
          }}
          className={cn(
            "rounded-s text-meta font-semibold text-primary-text hover:underline",
            focusRing,
          )}
        >
          {toggleLabel(expanded)}
        </button>
      ) : null}
    </div>
  );
}
