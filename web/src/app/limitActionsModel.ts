import { accountLabel } from "../features/settings/accountModel";

/** One option of the Switch account dropdown: the account the choice makes the project's, and its words. */
export interface SwitchItem {
  account: string;
  label: string;
}

/** The options of the usage-limit line's one Switch account dropdown, in the daemon's order, whatever their number; null without a target, which leaves the dropdown out. `system-default` reads as Claude Code's own login. */
export function switchItems(accounts: readonly string[]): SwitchItem[] | null {
  if (accounts.length === 0) {
    return null;
  }
  return accounts.map((account) => ({
    account,
    label: accountLabel(account),
  }));
}

/** The dropdown's name, which its first option shows until a person chooses. */
export const SWITCH_ACCOUNT = "Switch account";

/** The first option of the dropdown, which stays the shown value so no target is ever chosen ahead: `Switch account`, or `Switching to work-b…` while the daemon has not answered a switch. */
export function switchPlaceholder(pending: string | undefined): string {
  return pending === undefined ? SWITCH_ACCOUNT : `Switching to ${pending}…`;
}
