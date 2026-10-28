import { useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import {
  awaitingHumanQuery,
  needsYouQuery,
  sessionQuery,
} from "../api/queries";
import { Count } from "../components/badge/Chip";
import { Tooltip } from "../components/overlay/Tooltip";
import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { NAV_ENTRIES, activeNav, navPath, type NavId } from "../routes/nav";
import { HrefLink } from "./HrefLink";
import { reviewsWaitingFor } from "./needsYouModel";
import { tabBadge, type TabBadge } from "./tabBadges";

/** The project's tabs with the current one and their gold counts, for the top bar's row and the phone menu. */
export function useTabs(slug: string): {
  entries: typeof NAV_ENTRIES;
  active: NavId | null;
  badge: (id: NavId) => TabBadge | null;
} {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const { data: awaiting = 0 } = useQuery(awaitingHumanQuery(slug));
  const needs = useQuery(needsYouQuery(slug));
  const session = useQuery(sessionQuery());
  const reviews = reviewsWaitingFor(
    needs.data ?? [],
    session.data?.human ?? null,
  );
  return {
    entries: NAV_ENTRIES,
    active: activeNav(pathname),
    badge: (id) => tabBadge(id, reviews, awaiting),
  };
}

export function TabCount({ badge }: { badge: TabBadge }) {
  return (
    <Tooltip content={badge.hint}>
      <span tabIndex={-1} className="inline-flex">
        <Count value={badge.count} label={badge.label} />
      </span>
    </Tooltip>
  );
}

/** The tab row of the top bar on a wide screen. */
export function NavTabs({
  slug,
  className,
}: {
  slug: string;
  className?: string;
}) {
  const tabs = useTabs(slug);
  return (
    <nav aria-label="Project" className={cn("flex min-w-0 gap-2", className)}>
      {tabs.entries.map((entry) => {
        const current = entry.id === tabs.active;
        const badge = tabs.badge(entry.id);
        return (
          <HrefLink
            key={entry.id}
            href={navPath(slug, entry)}
            aria-current={current ? "page" : undefined}
            className={cn(
              "inline-flex items-center gap-6 rounded-s px-10 py-7 font-title text-15 whitespace-nowrap",
              current
                ? "bg-sunken font-semibold text-text"
                : "font-medium text-text-2 hover:bg-sunken hover:text-text",
              focusRing,
            )}
          >
            {entry.label}
            {badge === null ? null : <TabCount badge={badge} />}
          </HrefLink>
        );
      })}
    </nav>
  );
}
