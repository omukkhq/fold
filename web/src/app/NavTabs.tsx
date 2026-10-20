import { useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import {
  Activity,
  Bot,
  GitPullRequest,
  History,
  Inbox,
  Kanban,
  MessagesSquare,
  NotebookPen,
  Plus,
  type LucideIcon,
} from "lucide-react";
import { awaitingHumanQuery, notesUnreadQuery } from "../api/queries";
import { HrefLink } from "./HrefLink";
import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { buttonClasses } from "../components/button/buttonStyles";
import { TAB_LIST, tabClasses } from "../components/overlay/tabStyles";
import { newTaskPath } from "../features/board/card";
import { NAV_ENTRIES, activeNav, navPath, type NavId } from "../routes/nav";

const ICONS: Record<NavId, LucideIcon> = {
  board: Kanban,
  activity: Activity,
  proposals: GitPullRequest,
  requests: Inbox,
  agents: Bot,
  messages: MessagesSquare,
  notes: NotebookPen,
  repository: History,
};

export function NavTabs({ slug }: { slug: string }) {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const active = activeNav(pathname);
  const { data: awaiting = 0 } = useQuery(awaitingHumanQuery(slug));
  const { data: unreadNotes = null } = useQuery(notesUnreadQuery(slug));
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
            {entry.id === "messages" && awaiting > 0 ? (
              <span
                role="status"
                aria-label={`${String(awaiting)} awaiting a reply`}
                className="rounded-s bg-ink px-6 py-2 text-caption leading-snug font-semibold text-on-ink"
              >
                {awaiting}
              </span>
            ) : null}
            {entry.id === "notes" && unreadNotes !== null && unreadNotes > 0 ? (
              <span
                role="status"
                aria-label={`${String(unreadNotes)} unread notes`}
                className="rounded-s bg-ink px-6 py-2 text-caption leading-snug font-semibold text-on-ink"
              >
                {unreadNotes}
              </span>
            ) : null}
          </HrefLink>
        );
      })}
      {/* The create control sits at the end of the tab row on every screen; it opens the dialog over the board. */}
      <HrefLink
        href={newTaskPath(slug)}
        className={cn(buttonClasses("primary", "md"), focusRing, "ml-auto")}
      >
        <Plus size={15} aria-hidden />
        New task
      </HrefLink>
    </nav>
  );
}
