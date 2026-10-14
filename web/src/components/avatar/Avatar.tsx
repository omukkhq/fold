import { cn } from "@/lib/cn";
import { initials } from "./initials";
import { presenceSpec, type Presence } from "./presence";

interface AvatarProps {
  /** The agent or human name, for example `frontend/ui` or `human:tanim`. */
  name: string;
  className?: string;
}

export function Avatar({ name, className }: AvatarProps) {
  return (
    <span
      role="img"
      aria-label={name}
      className={cn(
        "inline-flex size-20 shrink-0 items-center justify-center rounded-full border border-primary bg-primary-soft text-micro font-bold text-primary-strong",
        className,
      )}
    >
      {initials(name)}
    </span>
  );
}

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
