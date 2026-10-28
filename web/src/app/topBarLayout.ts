import type { ConnectionState } from "../live/status";

/** The top bar shows its tabs, the Agents control and the avatar from the `wide` breakpoint (1440px). With the Settings tab, two-digit counts and a slug as wide as the switcher's 128px limit, the full row measured 1430px. Below, the tabs and those controls go into the menu, and Inbox and New task stay in the bar. */
export const WIDE_ONLY = "max-wide:hidden";

export const NARROW_ONLY = "wide:hidden";

/** The longest slug that fits the switcher's width limit in the title face; a longer one is cut. */
export const SWITCHER_FIT = 10;

/** The switcher's tooltip, which names a slug the switcher cuts; null for a slug that fits. */
export function switcherTooltip(slug: string): string | null {
  return slug.length > SWITCHER_FIT ? slug : null;
}

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
