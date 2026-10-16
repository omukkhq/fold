import type { CheckedState } from "@radix-ui/react-checkbox";

/** The check box: 16px, radius-s, a stroke while empty and the primary fill with an on-primary tick once checked. */
export const checkboxClasses =
  "inline-flex size-16 shrink-0 items-center justify-center rounded-s bg-surface text-on-primary inset-ring inset-ring-border disabled:cursor-not-allowed disabled:opacity-60 data-[state=checked]:bg-primary data-[state=checked]:inset-ring-primary";

/** The form value of a Radix checked state; the components never set the indeterminate state, so it reads as unchecked. */
export function isChecked(state: CheckedState): boolean {
  return state === true;
}
