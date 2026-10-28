import type { ComponentPropsWithoutRef } from "react";
import { HrefLink } from "@/app/HrefLink";
import { ID_LINK_CLASSES } from "@/components/id/idStyles";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";

/** A public id that opens its page inside the app, drawn as the shared id link. */
export function IdLink({
  className,
  ...rest
}: Omit<ComponentPropsWithoutRef<"a">, "href"> & { href: string }) {
  return (
    <HrefLink {...rest} className={cn(ID_LINK_CLASSES, focusRing, className)} />
  );
}
