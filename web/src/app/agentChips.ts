import type { ProjectAgent } from "../api/agentBar";
import { formatAsOfMinute } from "../format/time";
import type { LiveStatus } from "../live/status";
import { syncedAt } from "./streamStatus";

/** The top bar has room for three chips beside the stream status and the Theme button. */
export const MAX_CHIPS = 3;

/** The chip's second line from the agent's claims, or that no session is connected. */
export function agentStateText(agent: ProjectAgent): string {
  if (agent.presence === "offline") {
    return "not connected";
  }
  if (agent.working === 0) {
    return "waiting";
  }
  const working = `${String(agent.working)} working`;
  return agent.stalled > 0
    ? `${working} · ${String(agent.stalled)} stalled`
    : working;
}

/** While the stream is down the counts may be old, so the chip says when they were true; before the first connection it says nothing. */
export function chipStateText(
  agent: ProjectAgent,
  status: LiveStatus,
  now: number,
): string {
  if (status.state === "live") {
    return agentStateText(agent);
  }
  const at = status.state === "reconnecting" ? syncedAt(status) : null;
  return at === null ? "—" : `as of ${formatAsOfMinute(now, at)}`;
}

/**
 * Connected agents (active or idle) before offline ones, each group in the order the API lists them, at most `MAX_CHIPS`.
 * Active and idle share a group because presence flips between them every few minutes, and a chip keeps its place while it does.
 */
export function chipAgents(agents: readonly ProjectAgent[]): ProjectAgent[] {
  const connected = agents.filter((agent) => agent.presence !== "offline");
  const offline = agents.filter((agent) => agent.presence === "offline");
  return [...connected, ...offline].slice(0, MAX_CHIPS);
}

/** The board filtered to the agent's tasks, through the board's `agent` search param. */
export function agentBoardPath(slug: string, agent: string): string {
  return `/p/${slug}/board?${new URLSearchParams({ agent }).toString()}`;
}
