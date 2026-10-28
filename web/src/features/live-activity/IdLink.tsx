import type { ComponentProps } from "react";
import { HrefLink } from "../../app/HrefLink";
import { cn } from "../../lib/cn";
import { focusRing } from "../../lib/focus";

/** An id as a router link, in the body face with tabular numbers, as every id reads. */
export function IdLink({
  className,
  ...rest
}: ComponentProps<typeof HrefLink>) {
  return (
    <HrefLink
      {...rest}
      className={cn(
        "shrink-0 rounded-s font-body text-13 whitespace-nowrap text-text-2 tabular-nums underline-offset-2 hover:text-text hover:underline",
        focusRing,
        className,
      )}
    />
  );
}
