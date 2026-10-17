import { cn } from "@/lib/cn";
import { SKELETON_BARS } from "./skeletonModel";

interface SkeletonProps {
  className?: string;
}

/** A loading card. The shimmer stops under `prefers-reduced-motion`. */
export function Skeleton({ className }: SkeletonProps) {
  return (
    <div
      role="status"
      aria-busy="true"
      aria-label="Loading"
      className={cn(
        "flex flex-col gap-8 rounded-m bg-surface p-12 inset-ring inset-ring-border-subtle",
        className,
      )}
    >
      {SKELETON_BARS.map((bar, index) => (
        <div
          key={index}
          className={cn(
            "animate-pulse rounded-xs bg-sunken motion-reduce:animate-none",
            bar.height,
            bar.width,
          )}
        />
      ))}
    </div>
  );
}
