import { ActorText } from "@/people/ActorText";
import { HrefLink } from "@/app/HrefLink";
import type { ThreadView } from "@/api/messages";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { rowTime } from "./days";
import { rowSecond } from "./threadRow";
import { threadTitle } from "./title";

interface ThreadListProps {
  threads: ThreadView[];
  selected: string | null;
  now: number;
  you: string | null;
  hrefOf: (thread: ThreadView) => string;
}

/** The threads as two-line rows that fills separate: the title and its time, then who asks you or the newest message. */
export function ThreadList({
  threads,
  selected,
  now,
  you,
  hrefOf,
}: ThreadListProps) {
  return (
    <ul aria-label="Threads" className="flex flex-col gap-6 lg:gap-2">
      {threads.map((thread) => {
        const current = thread.id === selected;
        const second = rowSecond(thread, you);
        return (
          <li key={thread.id}>
            <HrefLink
              href={hrefOf(thread)}
              aria-current={current ? "true" : undefined}
              className={cn(
                "grid grid-cols-[minmax(0,1fr)_auto] gap-x-12 gap-y-3 rounded-m px-12 py-10",
                current ? "bg-accent-soft" : "max-lg:bg-surface lg:hover:bg-bg",
                focusRing,
              )}
            >
              <span className="truncate font-title text-16 leading-tight font-semibold text-text">
                {threadTitle(thread)}
              </span>
              <span className="self-baseline text-13 whitespace-nowrap text-text-2 tabular-nums">
                {rowTime(now, thread.last_at)}
              </span>
              <span className="col-span-2 flex min-w-0 items-center gap-8 text-14 text-text">
                {thread.awaiting_human ? (
                  <span
                    role="img"
                    aria-label="waits for you"
                    className="size-8 shrink-0 rounded-full bg-warn"
                  />
                ) : null}
                <span
                  className={cn(
                    "min-w-0 truncate",
                    second.asks && "font-semibold text-warn",
                  )}
                >
                  <ActorText
                    segments={second.segments}
                    personClassName="text-14"
                    you={you}
                  />
                </span>
              </span>
            </HrefLink>
          </li>
        );
      })}
    </ul>
  );
}
