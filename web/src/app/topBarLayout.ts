import type { ConnectionState } from "../live/status";

/** The top bar shows its tabs, the Agents control and the avatar from `xl` (1280px): the full row needs about 1250px. Below, the tabs and those controls go into the menu, and Inbox and New task stay in the bar. */
export const WIDE_ONLY = "max-xl:hidden";

export const NARROW_ONLY = "xl:hidden";

/** The states inside the phone menu that a person must not miss, as a mark on the menu button and the end of its name. */
export function menuAlert(
  stream: ConnectionState,
  paused: boolean,
): { label: string; dot: string } | null {
  if (stream === "reconnecting") {
    return { label: "Reconnecting", dot: "bg-warn" };
  }
  if (paused) {
    return { label: "Agents paused", dot: "bg-warn" };
  }
  return null;
}

export function menuLabel(alert: { label: string } | null): string {
  return alert === null ? "Menu" : `Menu, ${alert.label}`;
}
