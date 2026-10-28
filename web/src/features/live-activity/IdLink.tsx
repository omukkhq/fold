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
        "rounded-s font-body text-13 tabular-nums whitespace-nowrap text-text-2 underline-offset-2 hover:text-text hover:underline",
        focusRing,
        className,
      )}
    />
  );
}
