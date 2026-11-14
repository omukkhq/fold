/** One entry of the Switch account list: the account a choice makes the project's, and its words. */
export interface SwitchItem {
  account: string;
  label: string;
}

/** The entries of the usage-limit line's one Switch account list, by name in the daemon's order, whatever their number; null without a target, which leaves the control out. */
export function switchItems(accounts: readonly string[]): SwitchItem[] | null {
  if (accounts.length === 0) {
    return null;
  }
  return accounts.map((account) => ({ account, label: account }));
}

/** The words of the list's opener. */
export const SWITCH_ACCOUNT = "Switch account";

/** The words of the opener while the daemon has not answered a switch. */
export function switchPendingLabel(account: string): string {
  return `Switching to ${account}…`;
}
