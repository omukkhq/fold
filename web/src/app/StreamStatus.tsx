import { useLiveStatus } from "../live/status";
import { cn } from "../lib/cn";
import { streamView } from "./streamStatus";
import { useNow } from "../live/useNow";

export function StreamStatus() {
  const status = useLiveStatus();
  const now = useNow(status.state === "live");
  const view = streamView(status, now);
  return (
    <div className="flex items-center gap-6">
      <span
        aria-hidden
        className={cn("inline-block size-7 shrink-0 rounded-full", view.dot)}
      />
      <span className={cn("text-meta", view.tone)}>
        <span role="status">{view.label}</span>
        {view.detail !== null && ` · ${view.detail}`}
      </span>
    </div>
  );
}
