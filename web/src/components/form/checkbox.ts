import type { CheckedState } from "@radix-ui/react-checkbox";

/** The check box: 16px, radius-s, a secondary-tone stroke while empty, which keeps 3:1 against the surface, and the ink fill with an on-ink tick once checked. */
export const checkboxClasses =
  "inline-flex size-16 shrink-0 items-center justify-center rounded-s bg-surface text-on-ink inset-ring inset-ring-text-2 disabled:cursor-not-allowed disabled:opacity-60 data-[state=checked]:bg-ink data-[state=checked]:inset-ring-ink";

/** The form value of a Radix checked state; the components never set the indeterminate state, so it reads as unchecked. */
export function isChecked(state: CheckedState): boolean {
  return state === true;
}
