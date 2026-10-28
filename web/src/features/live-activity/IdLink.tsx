import type { ComponentProps } from "react";
import { HrefLink } from "../../app/HrefLink";
import { cn } from "../../lib/cn";
import { focusRing } from "../../lib/focus";

/** An id or a path as a router link, in the mono face. */
export function IdLink({
  className,
  ...rest
}: ComponentProps<typeof HrefLink>) {
  return (
    <HrefLink
      {...rest}
      className={cn(
        "shrink-0 rounded-s font-mono text-meta whitespace-nowrap text-text-muted tabular-nums underline-offset-2 hover:text-text hover:underline",
        focusRing,
        className,
      )}
    />
  );
}
