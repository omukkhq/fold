import { Fragment } from "react";
import { textFlow } from "@/components/text/textFlow";
import { cn } from "@/lib/cn";
import { MentionLink } from "./MentionLink";
import { mentionHref, splitMentions, type Known } from "./parse";
import { mentionTag } from "./remark";
import { usePeople } from "./usePeople";

interface MentionProps {
  text: string;
  slug: string;
  known?: Known;
}

/** The text with its mentions linked, inline in the caller's element; React escapes every string, so markup in the text shows as typed. */
export function MentionInline({ text, slug, known }: MentionProps) {
  const people = usePeople(slug);
  return splitMentions(text, known, people).map((segment, index) => (
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
        "text-15 leading-normal text-text",
        textFlow("prose"),
        className,
      )}
    >
      <MentionInline {...props} />
    </p>
  );
}
