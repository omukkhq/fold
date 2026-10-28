import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import type { ListNote } from "./listNote";

/** A panel's line in place of its list; a failed read is an alert with Retry. */
export function PanelNote({
  note,
  onRetry,
  className,
}: {
  note: ListNote;
  onRetry: () => void;
  className?: string;
}) {
  if (!note.failed) {
    return <p className={cn("text-14 text-text-2", className)}>{note.text}</p>;
  }
  return (
    <p
      role="alert"
      className={cn("flex items-center gap-8 text-14 text-text", className)}
    >
      {note.text}
      <button
        type="button"
        onClick={onRetry}
        className={cn("rounded-s text-accent underline", focusRing)}
      >
        Retry
      </button>
    </p>
  );
}
