import { CommitId } from "@/components/mono/CommitId";
import { CodeText } from "@/components/code/CodeText";
import { PersonName } from "@/people/PersonName";
import { cn } from "../../lib/cn";
import { eventGroups, liveDot } from "./eventStreamModel";
import { useEventStream } from "./useEventStream";

const DOT: Record<ReturnType<typeof liveDot>["tone"], string> = {
  live: "bg-ok",
  connecting: "bg-text-2",
  reconnecting: "bg-warn",
};

export function EventStream({ slug }: { slug: string }) {
  const stream = useEventStream(slug);
  const dot = liveDot(stream.live.state);
  return (
    <aside
      aria-label="Event stream"
      className="flex w-full shrink-0 flex-col overflow-hidden rounded-m bg-surface pt-14 pb-10 lg:w-event-stream"
    >
      <header className="flex items-center gap-8 px-18 pb-6">
        <h2 className="flex-1 text-16 font-semibold text-text">Event stream</h2>
        <span
          aria-hidden="true"
          className={cn("size-7 rounded-full", DOT[dot.tone])}
        />
        <span className="text-13 text-text-2">{dot.label}</span>
      </header>
      {stream.status === "pending" ? (
        <p role="status" className="px-18 py-8 text-14 text-text-2">
          Loading events
        </p>
      ) : null}
      {stream.status === "error" ? (
        <p
          role="alert"
          className="flex items-center gap-8 px-18 py-8 text-14 text-text-2"
        >
          Events could not load.
          <button
            type="button"
            onClick={stream.retry}
            className="rounded-s text-accent underline"
          >
            Retry
          </button>
        </p>
      ) : null}
      {stream.status === "ready" && stream.rows.length === 0 ? (
        <p className="px-18 py-8 text-14 text-text-2">
          No events yet. They appear here as agents work.
        </p>
      ) : null}
      {eventGroups(stream.rows).map((group) => (
        <section
          key={group.rows[0]?.seq}
          aria-label={group.minute}
          className="flex flex-col gap-4 px-18 pt-8 pb-4"
        >
          <h3 className="font-title text-13 leading-tight font-semibold text-text-2 tabular-nums">
            <time>{group.minute}</time>
          </h3>
          <ol className="flex flex-col gap-6">
            {group.rows.map((row) => (
              <li
                key={row.seq}
                className="grid grid-cols-[auto_minmax(0,1fr)] gap-10 text-14 leading-normal text-text"
              >
                <row.icon
                  aria-hidden="true"
                  size={14}
                  className={cn(
                    "mt-4",
                    row.emphasis ? "text-text" : "text-text-2",
                  )}
                />
                <p className="min-w-0 wrap-break-word">
                  <PersonName name={row.actor} /> <CodeText text={row.text} />
                  {row.commit === null ? null : " · "}
                  <CommitId commit={row.commit} />
                </p>
              </li>
            ))}
          </ol>
        </section>
      ))}
    </aside>
  );
}
