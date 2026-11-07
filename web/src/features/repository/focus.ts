/** The id of the stash panel's heading, where focus lands after a write removed the control that opened its dialog. */
export const STASH_HEADING_ID = "stash-heading";

/** The attribute on each Drop button that names its entry's place on the stack; the stash panel sets it as `data-stash-position`. */
const DROP_POSITION_ATTRIBUTE = "data-stash-position";

/** The Drop button that takes focus after the entry at `dropped` went: the entry now at that place, else the nearest newer one; null for the heading when no entry can be dropped. */
export function positionToFocus(
  positions: readonly number[],
  dropped: number,
): number | null {
  const after = positions.filter((position) => position >= dropped);
  if (after.length > 0) {
    return Math.min(...after);
  }
  return positions.length > 0 ? Math.max(...positions) : null;
}

/** Moves focus after a write to the folder or its stash: to the Drop button `positionToFocus` names when an entry was dropped, else to the stash heading. False when neither is on the page, so the dialog's own focus return runs. */
export function focusAfterFolderWrite(
  root: Document,
  dropped: number | null,
): boolean {
  const buttons = Array.from(
    root.querySelectorAll<HTMLElement>(`[${DROP_POSITION_ATTRIBUTE}]`),
  );
  const positions = buttons.map((button) =>
    Number(button.getAttribute(DROP_POSITION_ATTRIBUTE)),
  );
  const target = dropped === null ? null : positionToFocus(positions, dropped);
  const element =
    target === null
      ? root.getElementById(STASH_HEADING_ID)
      : buttons[positions.indexOf(target)];
  if (!element) {
    return false;
  }
  element.focus();
  return true;
}
