import { Fragment, type ReactNode } from "react";
import { codeParts } from "@/format/codeText";
import { cn } from "@/lib/cn";

/** A command inside a sentence: small mono text on the sunken surface, in the primary text colour even when the sentence is muted. In caption text it takes the caption size. */
export function InlineCode({
  children,
  caption = false,
}: {
  children: ReactNode;
  caption?: boolean;
}) {
  return (
    <code
      className={cn(
        "rounded-xs bg-sunken px-5 font-mono text-text [font-variant-ligatures:none]",
        caption ? "text-caption" : "text-code",
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
}: {
  text: string;
  caption?: boolean;
}) {
  return (
    <>
      {codeParts(text).map((part, index) => (
        <Fragment key={index}>
          {typeof part === "string" ? (
            part
          ) : (
            <InlineCode caption={caption}>{part.code}</InlineCode>
          )}
        </Fragment>
      ))}
    </>
  );
}
