import { Fragment } from "react";
import { HrefLink } from "@/app/HrefLink";
import { linkify } from "./linkify";

/** The body as text with linked ids; React escapes every string, so HTML in a body shows as typed. */
export function MessageBody({ body, slug }: { body: string; slug: string }) {
  return (
    <p className="text-body leading-normal whitespace-pre-wrap text-text">
      {linkify(body, slug).map((segment, index) => (
        <Fragment key={index}>
          {segment.href ? (
            <HrefLink
              href={segment.href}
              className="font-mono text-code text-primary-text hover:underline"
            >
              {segment.text}
            </HrefLink>
          ) : (
            segment.text
          )}
        </Fragment>
      ))}
    </p>
  );
}
