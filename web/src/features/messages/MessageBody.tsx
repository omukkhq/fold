import { Fragment } from "react";
import { HrefLink } from "@/app/HrefLink";
import { textFlow } from "@/components/text/textFlow";
import { cn } from "@/lib/cn";
import { MentionLink } from "@/mentions/MentionLink";
import { mentionHref, splitMentions } from "@/mentions/parse";
import { mentionTag } from "@/mentions/remark";
import { usePeople } from "@/mentions/usePeople";
import { linkify } from "./linkify";

/** The body as text with linked `#` and `@` mentions and bare ids; React escapes every string, so HTML in a body shows as typed. */
export function MessageBody({ body, slug }: { body: string; slug: string }) {
  const people = usePeople(slug);
  return (
    <p className={cn("text-body leading-normal text-text", textFlow("prose"))}>
      {splitMentions(body, undefined, people).map((part, index) => (
        <Fragment key={index}>
          {part.mention ? (
            <MentionLink
              href={mentionHref(slug, part.mention)}
              tag={mentionTag(part.mention)}
            >
              {part.text}
            </MentionLink>
          ) : (
            linkify(part.text, slug).map((segment, at) =>
              segment.href ? (
                <HrefLink
                  key={at}
                  href={segment.href}
                  className="font-mono text-code text-primary-text hover:underline"
                >
                  {segment.text}
                </HrefLink>
              ) : (
                <Fragment key={at}>{segment.text}</Fragment>
              ),
            )
          )}
        </Fragment>
      ))}
    </p>
  );
}
