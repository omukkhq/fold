import * as RadixPopover from "@radix-ui/react-popover";
import { useQuery } from "@tanstack/react-query";
import {
  Bell,
  ChevronRight,
  GitPullRequest,
  Hand,
  Inbox,
  MessagesSquare,
  ShieldAlert,
  UserRound,
  type LucideIcon,
} from "lucide-react";
import { useState } from "react";
import { needsYouQuery, sessionQuery } from "@/api/queries";
import type { NeedsYouKind } from "@/api/workers";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { useLiveStatus } from "@/live/status";
import { useNow } from "@/live/useNow";
import { PersonName } from "@/people/PersonName";
import { HrefLink } from "./HrefLink";
import {
  itemsFor,
  needsYouGroups,
  waitingText,
  type NeedsYouLine,
} from "./needsYou";

const ICONS: Record<NeedsYouKind, LucideIcon> = {
  question: MessagesSquare,
  permission: ShieldAlert,
  human_review: GitPullRequest,
  request_draft: Inbox,
  human_task: UserRound,
  blocker: Hand,
};

const MINUTE_MS = 60_000;

function Line({
  line,
  now,
  onOpen,
}: {
  line: NeedsYouLine;
  now: number;
  onOpen: () => void;
}) {
  const Icon = ICONS[line.kind];
  return (
    <li>
      <HrefLink
        href={line.href}
        onClick={onOpen}
        className={cn(
          "flex items-center gap-10 px-16 py-8 hover:bg-sunken",
          focusRing,
        )}
      >
        <Icon
          aria-hidden="true"
          size={15}
          className="shrink-0 text-text-muted"
        />
        <span className="flex min-w-0 flex-1 flex-col gap-2">
          <span className="flex items-center gap-5 text-body text-text">
            {line.person ? (
              <PersonName name={line.person} slot="needsYouLine" />
            ) : null}
            {line.chip ? (
              <span className="inline-flex items-center gap-4 rounded-s bg-sunken px-6 py-2 font-mono text-code text-primary-text">
                <Icon aria-hidden="true" size={12} />
                {line.chip}
              </span>
            ) : null}
            <span>{line.text}</span>
          </span>
          <span className="flex items-center gap-6 text-meta text-text-muted">
            {line.task ? (
              <span className="font-mono text-label">{line.task}</span>
            ) : null}
            {line.title ? (
              <span className="min-w-0 truncate">{line.title}</span>
            ) : null}
            <span>{formatAsOfMinute(now, line.since)}</span>
            <span className="text-text-faint">· {line.hint}</span>
          </span>
        </span>
        <ChevronRight
          aria-hidden="true"
          size={14}
          className="text-text-faint"
        />
      </HrefLink>
    </li>
  );
}

/** The top bar's count of what waits for a human, and the list behind it. Hidden while nothing waits. */
export function NeedsYou({ slug }: { slug: string }) {
  const needs = useQuery(needsYouQuery(slug));
  const session = useQuery(sessionQuery());
  const status = useLiveStatus();
  const now = useNow(status.state === "reconnecting", MINUTE_MS);
  const [open, setOpen] = useState(false);
  const items = itemsFor(needs.data ?? [], session.data?.human ?? null);
  if (items.length === 0) {
    return null;
  }
  const groups = needsYouGroups(slug, items);
  return (
    <RadixPopover.Root open={open} onOpenChange={setOpen}>
      <RadixPopover.Trigger asChild>
        <button
          type="button"
          className={cn(
            "inline-flex items-center gap-6 rounded-chip bg-warning-soft px-9 py-3 inset-ring inset-ring-border",
            focusRing,
          )}
        >
          <Bell aria-hidden="true" size={13} className="text-warning" />
          <span className="text-code font-medium text-text">Needs you</span>
          <span className="text-code font-semibold text-warning">
            {items.length}
          </span>
        </button>
      </RadixPopover.Trigger>
      <RadixPopover.Portal>
        <RadixPopover.Content
          align="end"
          sideOffset={6}
          aria-label="Needs you"
          className="z-10 flex w-needs-you max-w-full flex-col rounded-m border border-border bg-raised shadow-card outline-none"
        >
          <div className="flex items-center gap-8 border-b border-border-subtle px-16 py-12">
            <h2 className="text-card font-semibold text-text">Needs you</h2>
            <span className="text-code text-text-muted">
              {waitingText(items.length)}
            </span>
          </div>
          {groups.map((group) => (
            <section key={group.kind} aria-label={group.title}>
              <h3 className="px-16 pt-10 pb-2 text-caption font-semibold tracking-overline text-text-faint uppercase">
                {group.title}
              </h3>
              <ul>
                {group.lines.map((line) => (
                  <Line
                    key={line.key}
                    line={line}
                    now={now}
                    onOpen={() => {
                      setOpen(false);
                    }}
                  />
                ))}
              </ul>
            </section>
          ))}
          <p className="border-t border-border-subtle px-16 py-10 text-meta text-text-faint">
            Each item opens its own screen.
          </p>
        </RadixPopover.Content>
      </RadixPopover.Portal>
    </RadixPopover.Root>
  );
}
