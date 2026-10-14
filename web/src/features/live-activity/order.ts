export interface Orderable {
  id: string;
  lastActivityAt: number;
}

/**
 * New panels go on top, newest activity first; a panel already shown keeps its place, so a file change in it never moves it under the pointer.
 * `previous` is the order on screen.
 */
export function orderPanels(
  previous: readonly string[],
  items: readonly Orderable[],
): string[] {
  const present = new Set(items.map((item) => item.id));
  const shown = new Set(previous);
  const kept = previous.filter((id) => present.has(id));
  const fresh = items
    .filter((item) => !shown.has(item.id))
    .sort(
      (a, b) => b.lastActivityAt - a.lastActivityAt || a.id.localeCompare(b.id),
    )
    .map((item) => item.id);
  return [...fresh, ...kept];
}
