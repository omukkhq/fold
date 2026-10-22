/** The main agent's session is outside fold's view, and fold guesses no number for it. */
export const MAIN_AGENT_NOTE =
  "The main agent runs in your own Claude Code session, which fold does not supervise, so its cost is not in these numbers.";

/** The note under the cost, or `null` for none. A project whose `[main] supervised` setting is on has fold run the main agent as a worker, so its cost is in the numbers and the note would be wrong; `undefined`, while the workers list is not read yet, shows none either. */
export function mainAgentNote(
  mainSupervised: boolean | undefined,
): string | null {
  return mainSupervised === false ? MAIN_AGENT_NOTE : null;
}
