import { RefreshCw, WifiOff } from "lucide-react";
import { cn } from "@/lib/cn";
import { useNow } from "@/live/useNow";
import { Button } from "../button/Button";
import { reconnectingParts } from "./reconnecting";

interface ReconnectingBannerProps {
  /** The time of the state on screen, in UTC milliseconds. */
  asOf: number;
  /** The current time; with `retryAt` the banner reads the display clock instead, so the countdown moves each second. */
  now: number;
  /** When the next connection attempt starts, in UTC milliseconds; the banner then counts down to it. */
  retryAt?: number;
  /** What the screen shows, `the board`; the banner says `the state` without it. */
  shown?: string;
  onRetry?: () => void;
  className?: string;
}

export function ReconnectingBanner({
  asOf,
  now,
  retryAt,
  shown,
  onRetry,
  className,
}: ReconnectingBannerProps) {
  const counting = retryAt !== undefined;
  const clock = useNow(counting);
  const text = reconnectingParts(counting ? clock : now, asOf, retryAt, shown);
  return (
    <div
      role="status"
      className={cn(
        "flex items-center gap-10 bg-warn-soft px-24 py-9",
        className,
      )}
    >
      <WifiOff aria-hidden="true" size={16} className="shrink-0 text-warn" />
      <p className="min-w-0 flex-1 text-14 leading-tight text-text">
        {text.paused}{" "}
        {text.countdown !== null && (
          <span aria-hidden="true">{text.countdown} </span>
        )}
        {text.asOf}
      </p>
      {onRetry ? (
        <Button
          variant="ghost"
          size="sm"
          icon={<RefreshCw aria-hidden="true" size={14} />}
          onClick={onRetry}
        >
          Retry now
        </Button>
      ) : null}
    </div>
  );
}
