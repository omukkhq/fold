import { TriangleAlert } from "lucide-react";

/** The workspace state of the panel head: a dot and text on a soft fill. */
export function EditingPill({ text }: { text: string }) {
  return (
    <span className="inline-flex items-center gap-6 rounded-s bg-primary-soft px-8 py-2 text-caption font-medium text-primary-strong">
      <span aria-hidden="true" className="size-7 rounded-full bg-primary" />
      {text}
    </span>
  );
}

export function MissingBadge() {
  return (
    <span className="inline-flex items-center gap-5 rounded-s bg-danger-soft px-8 py-2 text-meta leading-snug font-semibold text-danger">
      <TriangleAlert aria-hidden="true" size={12} />
      missing
    </span>
  );
}
