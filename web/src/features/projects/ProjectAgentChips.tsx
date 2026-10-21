import { useLayoutEffect, useRef, useState } from "react";
import type { ProjectOverview } from "@/api/projects";
import { cn } from "@/lib/cn";
import { PersonName } from "@/people/PersonName";
import { fittingChips, moreLabel, moreTitle } from "./chipFit";

const CHIP =
  "flex shrink-0 items-center gap-5 rounded-full px-8 py-2 text-code whitespace-nowrap inset-ring inset-ring-border";

type Agent = ProjectOverview["agents"][number];

function Chip({ agent }: { agent: Agent }) {
  return (
    <span className={CHIP}>
      <PersonName name={agent.name} />
    </span>
  );
}

/** The connected agents of a project card on one line: as many chips as fit, then `+N` for the rest. A hidden copy of every chip is measured, so the line grows back when the card widens. */
export function ProjectAgentChips({
  agents,
  className,
}: {
  agents: readonly Agent[];
  className?: string;
}) {
  const line = useRef<HTMLDivElement>(null);
  const measure = useRef<HTMLDivElement>(null);
  const [shown, setShown] = useState(agents.length);
  useLayoutEffect(() => {
    const box = line.current;
    const copies = measure.current;
    if (!box || !copies) {
      return;
    }
    const fit = () => {
      const items = Array.from(copies.children, (item) =>
        item instanceof HTMLElement ? item.offsetWidth : 0,
      );
      const more = items.pop() ?? 0;
      const gap = Number.parseFloat(getComputedStyle(box).columnGap) || 0;
      setShown(fittingChips(items, box.clientWidth, gap, more));
    };
    fit();
    if (typeof ResizeObserver === "undefined") {
      return;
    }
    const observer = new ResizeObserver(fit);
    observer.observe(box);
    return () => {
      observer.disconnect();
    };
  }, [agents]);
  if (agents.length === 0) {
    return (
      <div className={cn("flex min-w-0 items-center", className)}>
        <span className="truncate text-code text-text-muted">
          no agents connected
        </span>
      </div>
    );
  }
  const hidden = agents.slice(shown).map((agent) => agent.name);
  return (
    <div
      ref={line}
      className={cn(
        "relative flex min-w-0 items-center gap-6 overflow-hidden",
        className,
      )}
    >
      {agents.slice(0, shown).map((agent) => (
        <Chip key={agent.name} agent={agent} />
      ))}
      {hidden.length > 0 ? (
        <span
          className={cn(CHIP, "text-text-muted")}
          title={moreTitle(hidden)}
          aria-label={moreTitle(hidden)}
        >
          {moreLabel(hidden.length)}
        </span>
      ) : null}
      <div
        ref={measure}
        aria-hidden
        className="invisible absolute top-0 left-0 flex w-max gap-6"
      >
        {agents.map((agent) => (
          <Chip key={agent.name} agent={agent} />
        ))}
        <span className={CHIP}>{moreLabel(agents.length)}</span>
      </div>
    </div>
  );
}
