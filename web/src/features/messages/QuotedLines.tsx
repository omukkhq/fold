import { FileCode } from "lucide-react";
import type { MessageLines } from "@/api/messages";
import { HrefLink } from "@/app/HrefLink";
import { CODE_BOX, textFlow } from "@/components/text/textFlow";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { cutNote, lineRefHref, lineRefText, quotedLines } from "./lineRef";

/** The lines of a proposal version a question to its author is about, as the version holds them, linked to the diff. */
export function QuotedLines({
  slug,
  proposal,
  lines,
}: {
  slug: string;
  proposal: string | null;
  lines: MessageLines;
}) {
  const head = lineRefText(lines);
  const note = cutNote(lines);
  return (
    <figure className="flex flex-col overflow-hidden rounded-s inset-ring inset-ring-border-subtle">
      <figcaption className="flex items-center gap-6 border-b border-border-subtle bg-bg px-10 py-6 font-mono text-caption text-text-muted">
        <FileCode aria-hidden="true" size={13} className="shrink-0" />
        {proposal === null ? (
          <span className="min-w-0 truncate">{head}</span>
        ) : (
          <HrefLink
            href={lineRefHref(slug, proposal, lines)}
            className={cn(
              "min-w-0 truncate rounded-s text-primary-text hover:underline",
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
          "bg-surface py-6 font-mono text-code leading-snug text-text",
        )}
      >
        {quotedLines(lines).map((line) => (
          <div key={line.number} className="flex">
            <span
              aria-hidden="true"
              className="w-diff-number shrink-0 pr-8 text-right text-text-faint select-none"
            >
              {line.number}
            </span>
            <code className={cn("pr-10", textFlow("code"))}>{line.text}</code>
          </div>
        ))}
      </pre>
      {note === null ? null : (
        <p className="border-t border-border-subtle px-10 py-4 text-meta text-text-muted">
          {note}
        </p>
      )}
    </figure>
  );
}
