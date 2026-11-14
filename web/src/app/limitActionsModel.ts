/** Up to this many switch targets show as one button each; more show as one Switch account menu, which keeps the line short on a phone. */
export const SWITCH_BUTTONS_UP_TO = 2;

/** How the usage-limit line offers its switch targets. */
export function switchLayout(count: number): "none" | "buttons" | "menu" {
  if (count === 0) {
    return "none";
  }
  return count <= SWITCH_BUTTONS_UP_TO ? "buttons" : "menu";
}

/** The words of the button that makes `account` the project's account. */
export function switchLabel(account: string): string {
  return `Switch to ${account}`;
}

/** The words of a switch while the daemon has not answered. */
export function switchPendingLabel(account: string): string {
  return `Switching to ${account}…`;
}

/** The line above the menu's accounts. */
export const SWITCH_MENU_HINT =
  "Make another account the project's: each waiting turn runs again on it, in its own session.";
