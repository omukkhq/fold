import { RefreshCw, WifiOff } from "lucide-react";
import { cn } from "@/lib/cn";
import { Button } from "../button/Button";
import { reconnectingText } from "./reconnecting";

interface ReconnectingBannerProps {
  /** The time of the state on screen, in UTC milliseconds. */
  asOf: number;
  /** The current time, passed in so the text is a pure function of its inputs. */
  now: number;
  /** When the next connection attempt starts, in UTC milliseconds; the banner then counts down to it. */
  retryAt?: number;
  onRetry?: () => void;
  className?: string;
}

export function ReconnectingBanner({
  asOf,
  now,
  retryAt,
  onRetry,
  className,
}: ReconnectingBannerProps) {
  return (
    // The bottom padding gives its last pixel to the border, as the frame's inner stroke does.
    <div
      role="status"
      className={cn(
        "flex items-center gap-10 border-b border-border-subtle bg-surface px-24 pt-9 pb-8",
        className,
      )}
    >
      <WifiOff
        aria-hidden="true"
        size={15}
        className="shrink-0 text-secondary-text"
      />
      <p className="min-w-0 flex-1 text-bar leading-snug text-text">
        {reconnectingText(now, asOf, retryAt)}
      </p>
      {onRetry ? (
        <Button
          variant="ghost"
          size="sm"
          icon={<RefreshCw aria-hidden="true" size={13} />}
          onClick={onRetry}
        >
          Retry now
        </Button>
      ) : null}
    </div>
  );
}
