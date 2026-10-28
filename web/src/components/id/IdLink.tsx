import type { ComponentProps } from "react";
import { HrefLink } from "@/app/HrefLink";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { ID_LINK_CLASSES } from "./idStyles";

/** A public id that opens its page inside the app through the router, so the page keeps its event stream and cache. */
export function IdLink({
  className,
  ...rest
}: ComponentProps<typeof HrefLink>) {
  return (
    <HrefLink {...rest} className={cn(ID_LINK_CLASSES, focusRing, className)} />
  );
}
