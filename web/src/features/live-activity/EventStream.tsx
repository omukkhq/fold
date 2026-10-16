import { CommitId } from "@/components/mono/CommitId";
import { PersonName } from "@/people/PersonName";
import { cn } from "../../lib/cn";
import { liveDot } from "./eventStream";
import { useEventStream } from "./useEventStream";

const DOT: Record<ReturnType<typeof liveDot>["tone"], string> = {
  live: "bg-success",
  connecting: "bg-text-faint",
  reconnecting: "bg-secondary",
};

export function EventStream({ slug }: { slug: string }) {
  const stream = useEventStream(slug);
  const dot = liveDot(stream.live.state);
  return (
    <aside
      aria-label="Event stream"
      className="flex w-full shrink-0 flex-col overflow-hidden rounded-l bg-surface inset-ring inset-ring-border-subtle lg:w-event-stream"
    >
      <header className="flex items-center gap-8 border-b border-border-subtle px-16 py-14">
        <h2 className="flex-1 text-section font-semibold text-text">
          Event stream
        </h2>
        <span
          aria-hidden="true"
          className={cn("size-7 rounded-full", DOT[dot.tone])}
        />
        <span className="text-meta text-text-muted">{dot.label}</span>
      </header>
      {stream.status === "pending" ? (
        <p role="status" className="px-16 py-14 text-bar text-text-muted">
          Loading events
        </p>
      ) : null}
      {stream.status === "error" ? (
        <p
          role="alert"
          className="flex items-center gap-8 px-16 py-14 text-bar text-text-muted"
        >
          Events could not load.
          <button
            type="button"
            onClick={stream.retry}
            className="rounded-s text-primary-text underline"
          >
            Retry
          </button>
        </p>
      ) : null}
      {stream.status === "ready" && stream.rows.length === 0 ? (
        <p className="px-16 py-14 text-bar text-text-muted">
          No events yet. They appear here as agents work.
        </p>
      ) : null}
      <ol className="flex flex-col px-8 py-6">
        {stream.rows.map((row) => (
          <li key={row.seq} className="flex gap-10 rounded-s p-8">
            <span className="flex h-16 w-14 shrink-0 items-center justify-center">
              <row.icon
                aria-hidden="true"
                size={14}
                className={row.emphasis ? "text-text" : "text-text-muted"}
              />
            </span>
            <div className="flex min-w-0 flex-1 flex-col gap-2">
              <div className="flex h-16 items-center gap-6">
                <span className="min-w-0 flex-1 truncate font-mono text-label text-text">
                  <PersonName name={row.actor} slot="eventActor" />
                </span>
                <time className="font-mono text-caption text-text-faint">
                  {row.time}
                </time>
              </div>
              <p
                className={cn(
                  "text-bar",
                  row.emphasis ? "text-text" : "text-text-muted",
                )}
              >
                {row.text}
                {row.commit === null ? null : " · "}
                <CommitId commit={row.commit} />
              </p>
            </div>
          </li>
        ))}
      </ol>
    </aside>
  );
}
