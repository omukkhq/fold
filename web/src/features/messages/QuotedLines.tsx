import { FileCode } from "lucide-react";
import type { MessageLines } from "@/api/messages";
import { HrefLink } from "@/app/HrefLink";
import { CODE_BOX, textFlow } from "@/components/text/textFlow";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  cutNote,
  lineRefHref,
  lineRefText,
  quotedLines,
  type Quoted,
} from "./lineRef";

/** The lines of a proposal version a question to its author is about, as the version holds them, linked to the diff. */
export function QuotedLines({
  slug,
  proposal,
  lines,
  onSurface = true,
}: {
  slug: string;
  proposal: string | null;
  lines: MessageLines;
  /** The quote sits on a surface, such as a message card, so it takes the fill tone to stand apart; a thread on the page tone shows it on a surface. */
  onSurface?: boolean;
}) {
  return (
    <LineQuote
      head={lineRefText(lines)}
      href={proposal === null ? null : lineRefHref(slug, proposal, lines)}
      lines={lines}
      onSurface={onSurface}
    />
  );
}

/** A numbered excerpt under a head that names where its lines are, linked to them when `href` is set. */
export function LineQuote({
  head,
  href,
  lines,
  onSurface = true,
}: {
  head: string;
  href: string | null;
  lines: Quoted;
  onSurface?: boolean;
}) {
  const note = cutNote(lines);
  return (
    <figure
      className={cn(
        "flex max-w-measure flex-col overflow-hidden rounded-m",
        onSurface ? "bg-fill" : "bg-surface",
      )}
    >
      <figcaption className="flex items-center gap-6 px-12 pt-8 pb-4 font-mono text-13 text-text-2">
        <FileCode aria-hidden="true" size={14} className="shrink-0" />
        {href === null ? (
          <span className="min-w-0 truncate">{head}</span>
        ) : (
          <HrefLink
            href={href}
            className={cn(
              "min-w-0 truncate rounded-s text-accent hover:underline",
              focusRing,
            )}
          >
            {head}
          </HrefLink>
        )}
      </figcaption>
      <pre
        className={cn(
          CODE_BOX,
          "pt-4 pb-10 font-mono text-13 leading-normal text-text",
        )}
      >
        {quotedLines(lines).map((line) => (
          <div key={line.number} className="flex">
            <span
              aria-hidden="true"
              className="w-diff-number shrink-0 pr-8 text-right text-text-2 select-none"
            >
              {line.number}
            </span>
            <code className={cn("pr-10", textFlow("code"))}>{line.text}</code>
          </div>
        ))}
      </pre>
      {note === null ? null : (
        <p className="px-12 pb-8 text-13 text-text-2">{note}</p>
      )}
    </figure>
  );
}
