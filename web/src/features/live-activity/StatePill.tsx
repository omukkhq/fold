import { TriangleAlert } from "lucide-react";

/** The workspace state of the panel head: a dot and text on a soft fill. */
export function EditingPill({ text }: { text: string }) {
  return (
    <span className="inline-flex items-center gap-6 rounded-s bg-accent-soft px-8 py-2 text-13 font-medium text-accent">
      <span aria-hidden="true" className="size-7 rounded-full bg-accent" />
      {text}
    </span>
  );
}

export function MissingBadge() {
  return (
    <span className="inline-flex items-center gap-5 rounded-s bg-bad-soft px-8 py-2 text-13 leading-tight font-semibold text-bad">
      <TriangleAlert aria-hidden="true" size={12} />
      missing
    </span>
  );
}
