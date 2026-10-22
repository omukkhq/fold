import { ActorText } from "@/people/ActorText";
import { HrefLink } from "@/app/HrefLink";
import type { ThreadView } from "@/api/messages";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { insetFocusRing } from "@/lib/focus";
import { rowLine, rowPreview } from "./threadRow";
import { threadTitle } from "./title";

interface ThreadListProps {
  threads: ThreadView[];
  selected: string | null;
  now: number;
  you: string | null;
  hrefOf: (thread: ThreadView) => string;
}

export function ThreadList({
  threads,
  selected,
  now,
  you,
  hrefOf,
}: ThreadListProps) {
  return (
    <ul aria-label="Threads" className="flex flex-col">
      {threads.map((thread) => {
        const last = thread.messages.at(-1);
        const current = thread.id === selected;
        const line = rowLine(thread, you);
        return (
          <li key={thread.id} className="border-b border-border-subtle">
            <HrefLink
              href={hrefOf(thread)}
              aria-current={current ? "true" : undefined}
              className={cn(
                "relative flex flex-col gap-5 px-16 py-12 hover:bg-sunken",
                current && "bg-primary-soft hover:bg-primary-soft",
                insetFocusRing,
              )}
            >
              {current ? (
                <span
                  aria-hidden="true"
                  className="absolute inset-y-0 left-0 w-2 bg-primary"
                />
              ) : null}
              <span className="flex items-baseline gap-8">
                <span className="min-w-0 flex-1 truncate text-body leading-snug font-semibold text-text">
                  {threadTitle(thread)}
                </span>
                <span className="shrink-0 text-meta text-text-faint">
                  {formatAsOfMinute(now, thread.last_at)}
                </span>
              </span>
              <span className="flex items-center gap-8 text-code text-text-faint">
                <span className="min-w-0 flex-1 truncate">
                  <ActorText
                    segments={line.segments}
                    personClassName="text-meta"
                    you={you}
                  />
                </span>
                {thread.awaiting_human ? (
                  <span
                    role="img"
                    aria-label="waits for you"
                    className="size-8 shrink-0 rounded-full bg-warning"
                  />
                ) : null}
              </span>
              {last ? (
                <span className="truncate text-bar text-text-muted">
                  <ActorText
                    segments={rowPreview(last, you, line.sender)}
                    you={you}
                  />
                </span>
              ) : null}
            </HrefLink>
          </li>
        );
      })}
    </ul>
  );
}
