import { MessageCircle } from "lucide-react";
import { ActorText } from "@/people/ActorText";
import { Badge } from "@/components/badge/Badge";
import { HrefLink } from "@/app/HrefLink";
import type { ThreadView } from "@/api/messages";
import { formatAsOf } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { previewLine, routeLine } from "./delivery";
import { threadTitle } from "./title";

interface ThreadListProps {
  threads: ThreadView[];
  selected: string | null;
  now: number;
  hrefOf: (thread: ThreadView) => string;
}

export function ThreadList({
  threads,
  selected,
  now,
  hrefOf,
}: ThreadListProps) {
  return (
    <ul
      aria-label="Threads"
      className="flex flex-col overflow-hidden rounded-l bg-surface inset-ring inset-ring-border-subtle"
    >
      {threads.map((thread) => {
        const first = thread.messages[0];
        const last = thread.messages.at(-1);
        const current = thread.id === selected;
        return (
          <li
            key={thread.id}
            className="border-b border-border-subtle last:border-b-0"
          >
            <HrefLink
              href={hrefOf(thread)}
              aria-current={current ? "true" : undefined}
              className={cn(
                "flex flex-col gap-4 px-14 py-12",
                current && "bg-primary-soft",
                focusRing,
              )}
            >
              <span className="flex items-center gap-8">
                <span
                  className={cn(
                    "min-w-0 flex-1 truncate text-body leading-snug text-text",
                    thread.awaiting_human ? "font-bold" : "font-semibold",
                  )}
                >
                  {threadTitle(thread)}
                </span>
                <span className="text-meta text-text-muted">
                  {formatAsOf(now, thread.last_at)}
                </span>
              </span>
              {thread.awaiting_human ? (
                <Badge
                  tone="needs-you"
                  icon={MessageCircle}
                  className="self-start"
                >
                  Waits for you
                </Badge>
              ) : null}
              <span className="flex items-center gap-8 font-mono text-label text-text-muted">
                <span className="min-w-0 flex-1 truncate">
                  {first ? (
                    <ActorText
                      segments={routeLine(first)}
                      personClassName="text-bar"
                    />
                  ) : (
                    ""
                  )}
                </span>
                {thread.task ? <span>{thread.task}</span> : null}
                {thread.proposal ? <span>{thread.proposal}</span> : null}
              </span>
              {last ? (
                <span className="truncate text-bar text-text-muted">
                  <ActorText segments={previewLine(last)} />
                </span>
              ) : null}
            </HrefLink>
          </li>
        );
      })}
    </ul>
  );
}
