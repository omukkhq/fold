import { useRouter } from "@tanstack/react-router";
import type { ComponentPropsWithoutRef } from "react";
import { isPlainLeftClick } from "../lib/navClick";

type Props = Omit<ComponentPropsWithoutRef<"a">, "href"> & { href: string };

/** Links to a path by string: a typed `Link` needs `to` to name a route in the generated tree, and most screen paths have no route there. */
export function HrefLink({ href, onClick, ...rest }: Props) {
  const router = useRouter();
  return (
    <a
      {...rest}
      href={href}
      onClick={(event) => {
        onClick?.(event);
        if (isPlainLeftClick(event) && rest.target === undefined) {
          event.preventDefault();
          void router.navigate({ href });
        }
      }}
    />
  );
}
