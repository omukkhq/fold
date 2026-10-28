import { useLiveStatus } from "../live/status";
import { cn } from "../lib/cn";
import { streamLabel, streamView } from "./streamStatusModel";
import { useNow } from "../live/useNow";

/** The connection state with the time of the last sync. `announce` is false where the bar's `StreamAnnouncer` already tells a screen reader. */
export function StreamStatus({ announce = true }: { announce?: boolean }) {
  const status = useLiveStatus();
  const now = useNow(status.state === "live");
  const view = streamView(status, now);
  return (
    <div className="flex items-center gap-6">
      <span
        aria-hidden
        className={cn("inline-block size-7 shrink-0 rounded-full", view.dot)}
      />
      <span className={cn("text-13", view.tone)}>
        <span role={announce ? "status" : undefined}>{view.label}</span>
        {view.detail !== null && ` · ${view.detail}`}
      </span>
    </div>
  );
}

/** The connection state for a screen reader, always in the top bar: the visible state sits in panels that are mostly closed. */
export function StreamAnnouncer() {
  const { state } = useLiveStatus();
  return (
    <span role="status" className="sr-only">
      {streamLabel(state)}
    </span>
  );
}
