import { apiErrorCode } from "@/api/errors";
import type { ProjectAccount } from "@/api/workers";
import { failureText } from "@/format/failure";
import type { Failure } from "../workers/startLead";

/** The account that stands for Claude Code's own login, which the daemon lists first. */
export const SYSTEM_DEFAULT = "system-default";

export interface AccountOption {
  value: string;
  label: string;
}

/** The words of an account in a list: `system-default` says that it is Claude Code's own login. */
export function accountLabel(name: string): string {
  return name === SYSTEM_DEFAULT ? `${name} (Claude Code's own login)` : name;
}

/** The choices of the select, in the daemon's order: `system-default` first, then the host's accounts. An account the config names that the host lacks stays shown, so the select never claims another choice. */
export function accountOptions(view: ProjectAccount): AccountOption[] {
  const options = view.accounts.map((name) => ({
    value: name,
    label: accountLabel(name),
  }));
  if (!view.accounts.includes(view.account)) {
    options.push({
      value: view.account,
      label: `${view.account} (not on this host)`,
    });
  }
  return options;
}

/** The select's value: the choice under way while one is saved, else the project's account. `pending` is `undefined` when no choice is under way. */
export function shownValue(
  view: ProjectAccount,
  pending: string | undefined,
): string {
  return pending ?? view.account;
}

/** The note under the select: while a choice is saved, what a change does, or how to add a first account of your own. */
export function accountHint(view: ProjectAccount, pending: boolean): string {
  if (pending) {
    return "Saving…";
  }
  if (!view.accounts.includes(view.account)) {
    return `This host has no account ${view.account}, so no worker can start. Add it with fld account add ${view.account}, or choose another one.`;
  }
  if (view.accounts.every((name) => name === SYSTEM_DEFAULT)) {
    return "This host has no account of its own yet. Run claude setup-token, then fld account add <name> with the token on stdin.";
  }
  return "After a change, each worker continues in its own session with the new account when its current turn ends, an idle one at once. A running fld claude session keeps its account until it starts again.";
}

/** A refused choice, worded for its code. */
export function accountFailure(error: unknown): Failure {
  const code = apiErrorCode(error);
  switch (code) {
    case "account_not_found":
      return {
        code,
        text: "This host has no such account any more. The list shows the accounts as they are now.",
      };
    case "invalid_account_name":
      return { code, text: "The daemon refused the account's name." };
    case "invalid_config":
      return {
        code,
        text: "The project's .fold/config.toml is not valid. Fix it, then choose the account again.",
      };
    case "accounts_io":
      return {
        code,
        text: "fold could not read the host's accounts. Check the accounts folder in fold's data folder.",
      };
    case "human_required":
      return { code, text: "Only a signed-in person chooses the account." };
    default:
      return { code, text: failureText(error) };
  }
}
