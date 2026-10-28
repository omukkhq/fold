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
    <div
      role="alert"
      className={cn(
        "flex items-center gap-10 bg-bad-soft px-24 py-9",
        className,
      )}
    >
      <TriangleAlert
        aria-hidden="true"
        size={16}
        className="shrink-0 text-bad"
      />
      <p className="min-w-0 flex-1 text-14 leading-tight text-text">
        <strong className="font-semibold">{title}</strong> {message}
      </p>
      {onRetry ? (
        <Button
          variant="ghost"
          size="sm"
          icon={<RefreshCw aria-hidden="true" size={14} />}
          onClick={onRetry}
        >
          Retry
        </Button>
      ) : null}
    </div>
  );
}
