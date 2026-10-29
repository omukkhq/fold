/** `neutral` sits on the fill tone; gold (`warn`) is only for what waits for a person or needs attention. */
export type ChipTone = "neutral" | "warn" | "ok" | "bad";

const TONES: Record<ChipTone, string> = {
  neutral: "bg-fill text-text",
  warn: "bg-warn-soft text-warn",
  ok: "bg-ok-soft text-ok",
  bad: "bg-bad-soft text-bad",
};

/** A short state word on a pill: 13px, no outline, never wrapping. */
export function chipClasses(tone: ChipTone): string {
  return `inline-flex items-center gap-6 rounded-pill px-9 py-2 text-13 leading-tight font-medium whitespace-nowrap ${TONES[tone]}`;
}

/** The count badge is the only place the 12px step is used. */
const COUNT_SHAPE =
  "inline-grid h-20 min-w-20 place-items-center rounded-pill px-6 font-title text-12 font-bold tabular-nums";

/** A count of things that wait for a person, in gold. */
export const COUNT_CLASSES = `${COUNT_SHAPE} bg-warn-soft text-warn`;

/** A count of what waits for no one, such as the filters that are on or unread messages, in the accent tone: it is never gold. */
export const FILTER_COUNT_CLASSES = `${COUNT_SHAPE} bg-accent-soft text-accent`;
