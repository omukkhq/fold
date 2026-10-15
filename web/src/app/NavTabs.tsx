import { useQuery } from "@tanstack/react-query";
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
import { messagesQuery } from "../api/queries";
import { unreadCount } from "../features/messages/filters";
import { HrefLink } from "./HrefLink";
import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { TAB_LIST, tabClasses } from "../components/overlay/tabStyles";
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
  const { data: unread = 0 } = useQuery({
    ...messagesQuery(slug),
    select: unreadCount,
  });
  return (
    <nav aria-label="Project" className={cn(TAB_LIST, "bg-surface px-24")}>
      {NAV_ENTRIES.map((entry) => {
        const Icon = ICONS[entry.id];
        const current = entry.id === active;
        return (
          <HrefLink
            key={entry.id}
            href={navPath(slug, entry)}
            aria-current={current ? "page" : undefined}
            className={cn(tabClasses(current), focusRing)}
          >
            <Icon size={15} aria-hidden />
            {entry.label}
            {entry.id === "messages" && unread > 0 ? (
              <span
                role="status"
                aria-label={`${String(unread)} unread`}
                className="rounded-s bg-ink px-6 py-2 text-caption leading-snug font-semibold text-on-ink"
              >
                {unread}
              </span>
            ) : null}
          </HrefLink>
        );
      })}
    </nav>
  );
}
