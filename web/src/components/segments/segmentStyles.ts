/** A row of segments on the fill track, as the diff change switch draws it; the current segment is a raised surface, never an outline. */
export const SEGMENT_LIST = "flex flex-wrap gap-2 rounded-s bg-fill p-3";

export function segmentClasses(current: boolean): string {
  return current
    ? "rounded-s px-12 py-5 font-title text-14 leading-tight bg-surface font-semibold text-text shadow-pop"
    : "rounded-s px-12 py-5 font-title text-14 leading-tight font-semibold text-text-2 hover:text-text";
}
