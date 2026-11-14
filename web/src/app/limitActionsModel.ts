import { accountLabel } from "../features/settings/accountModel";

/** One entry of the Switch account list: the account a choice picks, and its words. */
export interface SwitchItem {
  account: string;
  label: string;
}

/** The entries of the usage-limit line's one Switch account list, in the daemon's order, whatever their number; null without a target, which leaves the control out. `system-default` reads as Claude Code's own login. */
export function switchItems(accounts: readonly string[]): SwitchItem[] | null {
  if (accounts.length === 0) {
    return null;
  }
  return accounts.map((account) => ({
    account,
    label: accountLabel(account),
  }));
}

/** The chosen account while it is still a target; null before a choice, and once a new read of the item no longer lists it. */
export function chosenTarget(
  chosen: string | null,
  accounts: readonly string[],
): string | null {
  return chosen !== null && accounts.includes(chosen) ? chosen : null;
}

/** The words of the list's trigger: `Switch account` before a choice, then the chosen account. */
export function triggerLabel(chosen: string | null): string {
  return chosen === null ? "Switch account" : accountLabel(chosen);
}

/** The name of the switch button, which only a choice enables. */
export function switchButtonLabel(chosen: string | null): string {
  return chosen === null ? "Choose an account first" : `Switch to ${chosen}`;
}

/** The words of the control while the daemon has not answered a switch. */
export function switchPendingLabel(account: string): string {
  return `Switching to ${account}…`;
}
