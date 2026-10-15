import { Fragment } from "react";
import { cn } from "@/lib/cn";
import { MentionLink } from "./MentionLink";
import { mentionHref, splitMentions, type Known } from "./parse";
import { mentionTag } from "./remark";

interface MentionProps {
  text: string;
  slug: string;
  known?: Known;
}

/** The text with its mentions linked, inline in the caller's element; React escapes every string, so markup in the text shows as typed. */
export function MentionInline({ text, slug, known }: MentionProps) {
  return splitMentions(text, known).map((segment, index) => (
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
  ));
}

/** Plain text with its mentions linked, as a paragraph of body text. */
export function MentionText({
  className,
  ...props
}: MentionProps & { className?: string }) {
  return (
    <p
      className={cn(
        "text-body leading-normal whitespace-pre-wrap text-text",
        className,
      )}
    >
      <MentionInline {...props} />
    </p>
  );
}
