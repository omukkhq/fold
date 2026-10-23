import type { ConnectionState } from "../live/status";

export type TopBarItem =
  | "stream"
  | "pause"
  | "needsYou"
  | "agents"
  | "bell"
  | "name"
  | "theme";

/** The order of the right side of the top bar on a wide screen. */
export const TOP_BAR_ITEMS: readonly TopBarItem[] = [
  "stream",
  "pause",
  "needsYou",
  "agents",
  "bell",
  "name",
  "theme",
];

/** Below `lg` (1024px) the full row does not fit, so the row keeps the wordmark, the project switcher, Needs you and three 44px buttons, which leaves the project name room at 360px; these items go behind the More button. */
const MORE_ITEMS: ReadonlySet<TopBarItem> = new Set([
  "stream",
  "pause",
  "agents",
  "theme",
]);

export function inMore(item: TopBarItem): boolean {
  return MORE_ITEMS.has(item);
}

/** The items behind the More button, in the order of the row. */
export function moreItems(): TopBarItem[] {
  return TOP_BAR_ITEMS.filter(inMore);
}

/** The classes that keep an item in the row only where it fits. */
export function rowItemClass(item: TopBarItem): string {
  return inMore(item) ? "max-lg:hidden" : "";
}

/** The More button exists only where items leave the row. */
export const MORE_TRIGGER_CLASS = "lg:hidden";

/** The states behind the More button that a person must not miss, as a mark on the button and the end of its name. */
export function moreAlert(
  stream: ConnectionState,
  paused: boolean,
): { label: string; dot: string } | null {
  if (stream === "reconnecting") {
    return { label: "Reconnecting", dot: "bg-secondary" };
  }
  if (paused) {
    return { label: "Agents paused", dot: "bg-warning" };
  }
  return null;
}

export function moreLabel(alert: { label: string } | null): string {
  return alert === null ? "More" : `More, ${alert.label}`;
}
