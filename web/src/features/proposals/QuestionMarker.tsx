import { MessageCircleQuestion } from "lucide-react";
import { HrefLink } from "@/app/HrefLink";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";

interface QuestionMarkerProps {
  href: string;
  /** Names the question, its lines and whether it waits, for a screen reader and as the tooltip. */
  label: string;
  answered: boolean;
  /** Only one line of a question's lines takes Tab focus. */
  focusable: boolean;
}

/** The mark at a line that a question to the author covers; it opens the question's thread. */
export function QuestionMarker({
  href,
  label,
  answered,
  focusable,
}: QuestionMarkerProps) {
  return (
    <HrefLink
      href={href}
      aria-label={label}
      title={label}
      tabIndex={focusable ? 0 : -1}
      // The row under the marker selects its line on a click.
      onClick={(event) => {
        event.stopPropagation();
      }}
      className={cn(
        "inline-flex rounded-s",
        answered ? "text-text-muted" : "text-secondary-text",
        focusRing,
      )}
    >
      <MessageCircleQuestion aria-hidden="true" size={13} />
    </HrefLink>
  );
}
