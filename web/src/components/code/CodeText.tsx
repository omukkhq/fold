import { Fragment, type ReactNode } from "react";
import { codeParts } from "@/format/codeText";
import { cn } from "@/lib/cn";

/** A command inside a sentence: small mono text on the sunken surface, in the primary text colour even when the sentence is muted, and never broken across lines. In caption text it takes the caption size. A sentence that itself sits on the sunken surface draws its commands on the plain surface instead, or the chip would vanish and leave only its padding as a gap. */
export function InlineCode({
  children,
  caption = false,
  onSunken = false,
}: {
  children: ReactNode;
  caption?: boolean;
  onSunken?: boolean;
}) {
  return (
    <code
      className={cn(
        "rounded-s px-5 font-mono whitespace-nowrap text-text [font-variant-ligatures:none]",
        onSunken ? "bg-surface" : "bg-sunken",
        caption ? "text-13" : "text-13",
      )}
    >
      {children}
    </code>
  );
}

/** A sentence that marks its commands between backticks, drawn with each command as inline code. Selecting the sentence copies the plain words. */
export function CodeText({
  text,
  caption = false,
  onSunken = false,
}: {
  text: string;
  caption?: boolean;
  /** The sentence sits on the sunken surface. */
  onSunken?: boolean;
}) {
  return (
    <>
      {codeParts(text).map((part, index) => (
        <Fragment key={index}>
          {typeof part === "string" ? (
            part
          ) : (
            <InlineCode caption={caption} onSunken={onSunken}>
              {part.code}
            </InlineCode>
          )}
        </Fragment>
      ))}
    </>
  );
}
