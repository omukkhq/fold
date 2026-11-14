import { accountLabel } from "../features/settings/accountModel";

/** One item of the Switch account menu: the account the choice makes the project's, and its words. */
export interface SwitchItem {
  account: string;
  label: string;
}

/** The items of the usage-limit line's one Switch account menu, in the daemon's order, whatever their number; null without a target, which leaves the menu out. `system-default` reads as Claude Code's own login. */
export function switchItems(accounts: readonly string[]): SwitchItem[] | null {
  if (accounts.length === 0) {
    return null;
  }
  return accounts.map((account) => ({
    account,
    label: accountLabel(account),
  }));
}

/** The words of the menu's trigger while the daemon has not answered a switch. */
export function switchPendingLabel(account: string): string {
  return `Switching to ${account}…`;
}

/** The line above the menu's accounts. */
export const SWITCH_MENU_HINT =
  "Make another account the project's: each waiting turn runs again on it, in its own session.";
