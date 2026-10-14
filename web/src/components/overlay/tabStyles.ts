/**
 * A tab draws its 2px underline inside its own box, over the bottom padding, as the frames do; a CSS border would add 2px to the height.
 */
const BASE =
  "relative flex items-center gap-6 px-2 py-13 text-body leading-snug after:absolute after:inset-x-0 after:bottom-0 after:h-2";

/**
 * The row of tabs. Its 1px divider is drawn inside the row and under the tabs, so the current tab's underline covers it, as in W02.
 */
export const TAB_LIST =
  "relative flex items-center gap-22 before:absolute before:inset-x-0 before:bottom-0 before:h-px before:bg-border-subtle";

/** The same tab for Radix, which marks the current trigger with `data-state=active`. */
export const TAB_BY_STATE = `${BASE} font-medium text-text-muted after:bg-transparent hover:text-text data-[state=active]:font-semibold data-[state=active]:text-text data-[state=active]:after:bg-ink`;

export function tabClasses(current: boolean): string {
  return current
    ? `${BASE} font-semibold text-text after:bg-ink`
    : `${BASE} font-medium text-text-muted after:bg-transparent hover:text-text`;
}
