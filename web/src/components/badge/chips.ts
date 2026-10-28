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

/** A count of things that wait for a person, in gold: the only place the 12px step is used. */
export const COUNT_CLASSES =
  "inline-grid h-20 min-w-20 place-items-center rounded-pill bg-warn-soft px-6 font-title text-12 font-bold text-warn tabular-nums";
