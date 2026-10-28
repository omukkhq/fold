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
    // The bottom padding is 1px less than the top, so the 1px border stays inside the banner's height like an inner stroke.
    <div
      role="alert"
      className={cn(
        "flex items-center gap-10 border-b border-line bg-surface px-24 pt-9 pb-8",
        className,
      )}
    >
      <TriangleAlert
        aria-hidden="true"
        size={15}
        className="shrink-0 text-bad"
      />
      <p className="min-w-0 flex-1 text-14 leading-tight text-text">
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
