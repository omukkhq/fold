import type { ComponentProps } from "react";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { ID_CLASSES, ID_LINK_CLASSES } from "./idStyles";

type IdProps =
  | ({ href?: undefined } & ComponentProps<"span">)
  | ({ href: string } & ComponentProps<"a">);

/** A public id such as `T-12`, `P-3` or `r9` in the body face with tabular numbers, never wrapping; with `href` it is a link. */
export function Id(props: IdProps) {
  if (props.href !== undefined) {
    const { className, ...rest } = props;
    return (
      <a {...rest} className={cn(ID_LINK_CLASSES, focusRing, className)} />
    );
  }
  const { className, ...rest } = props;
  return <span {...rest} className={cn(ID_CLASSES, className)} />;
}
