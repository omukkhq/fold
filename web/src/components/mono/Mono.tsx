import type { ComponentProps } from "react";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";

type MonoProps =
  | ({ href?: undefined } & ComponentProps<"span">)
  | ({ href: string } & ComponentProps<"a">);

/** A path, a command or a commit in the mono face; with `href` it is a link. Public ids use `Id`. */
export function Mono(props: MonoProps) {
  if (props.href !== undefined) {
    const { className, ...rest } = props;
    return (
      <a
        {...rest}
        className={cn(
          "rounded-s font-mono text-13 text-accent underline-offset-2 hover:underline",
          focusRing,
          className,
        )}
      />
    );
  }
  const { className, ...rest } = props;
  return <span {...rest} className={cn("font-mono text-13", className)} />;
}
