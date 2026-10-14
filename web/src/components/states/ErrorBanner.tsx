import { RefreshCw, TriangleAlert } from "lucide-react";
import { cn } from "@/lib/cn";
import { Button } from "../button/Button";

interface ErrorBannerProps {
  title: string;
  message: string;
  /** Shows a Retry button when given. */
  onRetry?: () => void;
  className?: string;
}

export function ErrorBanner({
  title,
  message,
  onRetry,
  className,
}: ErrorBannerProps) {
  return (
    // The bottom padding gives its last pixel to the border, as the frame's inner stroke does.
    <div
      role="alert"
      className={cn(
        "flex items-center gap-10 border-b border-border-subtle bg-surface px-24 pt-9 pb-8",
        className,
      )}
    >
      <TriangleAlert
        aria-hidden="true"
        size={15}
        className="shrink-0 text-danger"
      />
      <p className="min-w-0 flex-1 text-bar leading-snug text-text">
        <strong className="font-semibold">{title}</strong> {message}
      </p>
      {onRetry ? (
        <Button
          variant="ghost"
          size="sm"
          icon={<RefreshCw aria-hidden="true" size={13} />}
          onClick={onRetry}
        >
          Retry
        </Button>
      ) : null}
    </div>
  );
}
