/** A row of segments on a sunken track, as the diff change switch draws it. */
export const SEGMENT_LIST = "flex flex-wrap gap-2 rounded-m bg-sunken p-3";

export function segmentClasses(current: boolean): string {
  return current
    ? "rounded-s px-12 py-5 text-bar leading-snug bg-surface font-semibold text-text inset-ring inset-ring-border-subtle"
    : "rounded-s px-12 py-5 text-bar leading-snug font-medium text-text-muted hover:text-text";
}
