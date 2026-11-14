import { apiErrorCode } from "@/api/errors";
import type { ProjectAccount } from "@/api/workers";
import { failureText } from "@/format/failure";
import type { Failure } from "../workers/startLead";

/** The reserved account that stands for Claude Code's own login, which the daemon lists first when the host has that login. */
export const DEFAULT_ACCOUNT = "default";

export interface AccountOption {
  value: string;
  label: string;
}

/** The choices of the select, in the daemon's order: `default` first when the daemon lists it, then the host's accounts. An account the config names that the host lacks stays shown, so the select never claims another choice. */
export function accountOptions(view: ProjectAccount): AccountOption[] {
  const options = view.accounts.map((name) => ({ value: name, label: name }));
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
    return view.account === DEFAULT_ACCOUNT
      ? "This host has no Claude Code login, so no worker can start. Run claude /login, or choose another account."
      : `This host has no account ${view.account}, so no worker can start. Add it with fld account add ${view.account}, or choose another one.`;
  }
  if (view.accounts.every((name) => name === DEFAULT_ACCOUNT)) {
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
