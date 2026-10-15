import { Fragment } from "react";
import { cn } from "@/lib/cn";
import { MentionLink } from "./MentionLink";
import { mentionHref, splitMentions, type Known } from "./parse";
import { mentionTag } from "./remark";

/** Plain text with its mentions linked; React escapes every string, so markup in the text shows as typed. */
export function MentionText({
  text,
  slug,
  known,
  className,
}: {
  text: string;
  slug: string;
  known?: Known;
  className?: string;
}) {
  return (
    <p
      className={cn(
        "text-body leading-normal whitespace-pre-wrap text-text",
        className,
      )}
    >
      {splitMentions(text, known).map((segment, index) => (
        <Fragment key={index}>
          {segment.mention ? (
            <MentionLink
              href={mentionHref(slug, segment.mention)}
              tag={mentionTag(segment.mention)}
            >
              {segment.text}
            </MentionLink>
          ) : (
            segment.text
          )}
        </Fragment>
      ))}
    </p>
  );
}
