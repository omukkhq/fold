import { useRouterState } from "@tanstack/react-router";
import {
  Activity,
  Bot,
  GitPullRequest,
  History,
  Kanban,
  MessagesSquare,
  type LucideIcon,
} from "lucide-react";
import { HrefLink } from "./HrefLink";
import { cx } from "../lib/cx";
import { NAV_ENTRIES, activeNav, navPath, type NavId } from "../routes/nav";

const ICONS: Record<NavId, LucideIcon> = {
  board: Kanban,
  activity: Activity,
  proposals: GitPullRequest,
  agents: Bot,
  messages: MessagesSquare,
  repository: History,
};

export function NavTabs({ slug }: { slug: string }) {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const active = activeNav(pathname);
  return (
    <nav
      aria-label="Project"
      className="flex items-center gap-22 border-b border-border-subtle bg-surface px-24"
    >
      {NAV_ENTRIES.map((entry) => {
        const Icon = ICONS[entry.id];
        const current = entry.id === active;
        return (
          <HrefLink
            key={entry.id}
            href={navPath(slug, entry)}
            aria-current={current ? "page" : undefined}
            className={cx(
              "flex items-center gap-6 border-b-2 px-2 py-13 text-body outline-primary focus-visible:outline-2",
              current
                ? "border-ink font-semibold text-text"
                : "border-transparent font-medium text-text-muted hover:text-text",
            )}
          >
            <Icon size={15} aria-hidden />
            {entry.label}
          </HrefLink>
        );
      })}
    </nav>
  );
}
