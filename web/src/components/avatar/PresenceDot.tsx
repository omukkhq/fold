import { cn } from "@/lib/cn";
import { presenceSpec, type Presence } from "./presence";

interface PresenceDotProps {
  state: Presence;
  className?: string;
}

export function PresenceDot({ state, className }: PresenceDotProps) {
  const spec = presenceSpec(state);
  return (
    <span
      role="img"
      aria-label={spec.label}
      className={cn(
        "inline-block size-6 shrink-0 rounded-full",
        spec.dot,
        className,
      )}
    />
  );
}
