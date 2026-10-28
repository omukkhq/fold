import type { ComponentProps } from "react";
import { cn } from "@/lib/cn";
import { IdLink } from "./IdLink";
import { ID_CLASSES } from "./idStyles";

type IdProps =
  | ({ href?: undefined } & ComponentProps<"span">)
  | ({ href: string } & ComponentProps<typeof IdLink>);

/** A public id such as `T-12`, `P-3` or `r9` in the body face with tabular numbers, never wrapping; with `href` it is a link inside the app. */
export function Id(props: IdProps) {
  if (props.href !== undefined) {
    return <IdLink {...props} />;
  }
  const { className, ...rest } = props;
  return <span {...rest} className={cn(ID_CLASSES, className)} />;
}
