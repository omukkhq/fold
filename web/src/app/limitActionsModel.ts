/** The words of the button that makes `account` the project's account. */
export function switchLabel(account: string): string {
  return `Switch to ${account}`;
}

/** The words of that button while the daemon has not answered. */
export function switchPendingLabel(account: string): string {
  return `Switching to ${account}…`;
}
