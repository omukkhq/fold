import type { ProjectAgent } from "../api/agentBar";
import { formatAsOfMinute } from "../format/time";
import type { LiveStatus } from "../live/status";
import { syncedAt } from "./streamStatusModel";
import { projectPath, withQuery } from "@/routes/paths";

/** The top bar has room for three chips beside the stream status and the Theme button. */
export const MAX_CHIPS = 3;

/** Tasks the agent's workers hold an implementation claim on. */
export function workingCount(agent: ProjectAgent): number {
  return agent.workers.reduce(
    (count, worker) =>
      count + worker.claims.filter((c) => c.kind === "implementation").length,
    0,
  );
}

/** Tasks with a stalled worker. */
export function stalledCount(agent: ProjectAgent): number {
  return agent.attention.filter((item) => item.kind === "stalled").length;
}

type AgentProcess = NonNullable<ProjectAgent["process"]>;

/** The short words for a failure reason a person can act on; any other failure reads plain `failed`. */
const FAILED_REASONS: Readonly<Record<string, string>> = {
  mcp_unavailable: "no fold connection",
  fld_missing: "fld not found",
};

/** `failed`, followed by the reason when the chip has words for it. */
export function failedText(reason: string | null): string {
  const words = reason === null ? undefined : FAILED_REASONS[reason];
  return words === undefined ? "failed" : `failed: ${words}`;
}

/** `text`, followed by the count of stalled tasks when there are any. */
function withStalled(agent: ProjectAgent, text: string): string {
  const stalled = stalledCount(agent);
  return stalled > 0 ? `${text} · ${String(stalled)} stalled` : text;
}

/** What the chip says of a process whose turns fail before they reach the model, which the daemon decides. */
export const MODEL_UNREACHABLE_TEXT = "cannot reach the model";

/** Whether the process failed for a reason the chip has words for, which the human fixes before anything else. */
function knownFailure(process: AgentProcess): boolean {
  return (
    process.state === "failed" &&
    process.failed_reason !== null &&
    FAILED_REASONS[process.failed_reason] !== undefined
  );
}

/** A supervised agent runs Claude Code only during a turn, so its process tells its state; its presence would read offline between turns. A process whose turns cannot reach the model says so, unless it failed for a reason the chip names. */
function processText(agent: ProjectAgent, process: AgentProcess): string {
  if (process.model_unreachable && !knownFailure(process)) {
    return MODEL_UNREACHABLE_TEXT;
  }
  switch (process.state) {
    case "working": {
      const working = workingCount(agent);
      return withStalled(
        agent,
        working === 0 ? "working" : `${String(working)} working`,
      );
    }
    case "idle":
      return withStalled(agent, "idle");
    case "failed":
      return failedText(process.failed_reason);
    default:
      return process.state;
  }
}

/** The process the chip follows: the one fold supervises for the agent, unless that process has ended while a session the user started is connected as the agent. */
function chipProcess(agent: ProjectAgent): AgentProcess | null {
  const { process } = agent;
  if (process === null) {
    return null;
  }
  const ended = process.state === "stopped" || process.state === "failed";
  return ended && agent.presence !== "offline" ? null : process;
}

/** The chip's second line: the state of the process fold supervises for the agent; for an agent fold does not run, or whose process ended while a session is connected, its claims, or that no session is connected. */
export function agentStateText(agent: ProjectAgent): string {
  const process = chipProcess(agent);
  if (process !== null) {
    return processText(agent, process);
  }
  if (agent.presence === "offline") {
    return "not connected";
  }
  const working = workingCount(agent);
  return working === 0
    ? "waiting"
    : withStalled(agent, `${String(working)} working`);
}

/** The chip's tooltip: Claude Code's last error of the process the chip follows; undefined without one. */
export function chipErrorTitle(agent: ProjectAgent): string | undefined {
  return chipProcess(agent)?.last_error ?? undefined;
}

/** Whether a pause holds the agent: every agent but the main agent, whose session fold does not stop. */
function pausedAgent(agent: ProjectAgent, paused: boolean): boolean {
  return paused && !agent.tags.includes("main");
}

/** The dot beside the name: the state of the process the chip follows as a presence, so a lead between turns shows idle instead of offline; for any other agent, its presence. A pause ends every worker process but keeps its row's state, so a paused agent's dot is offline. */
export function chipPresence(
  agent: ProjectAgent,
  paused: boolean,
): ProjectAgent["presence"] {
  if (pausedAgent(agent, paused)) {
    return "offline";
  }
  switch (chipProcess(agent)?.state) {
    case undefined:
      return agent.presence;
    case "starting":
    case "working":
      return "active";
    case "idle":
    case "stopping":
      return "idle";
    default:
      return "offline";
  }
}

/** Whether the project's agents are paused, from the overview's `paused_since`; `undefined` while the project is not read yet. */
export function isPaused(pausedSince: number | null | undefined): boolean {
  return pausedSince !== null && pausedSince !== undefined;
}

/** While the stream is down the counts may be old, so the chip says when they were true; before the first connection it says nothing. While the project is paused, fold runs no worker process, so every agent but the main agent, whose session fold does not stop, reads `paused`. */
export function chipStateText(
  agent: ProjectAgent,
  status: LiveStatus,
  now: number,
  paused: boolean,
): string {
  if (status.state === "live") {
    return pausedAgent(agent, paused) ? "paused" : agentStateText(agent);
  }
  const at = status.state === "reconnecting" ? syncedAt(status) : null;
  return at === null ? "—" : `as of ${formatAsOfMinute(now, at)}`;
}

/**
 * Connected agents (active or idle, by `chipPresence`) before offline ones, each group in the order the API lists them, at most `MAX_CHIPS`.
 * Active and idle share a group because presence flips between them every few minutes, and a chip keeps its place while it does.
 */
export function chipAgents(
  agents: readonly ProjectAgent[],
  paused: boolean,
): ProjectAgent[] {
  const connected = agents.filter(
    (agent) => chipPresence(agent, paused) !== "offline",
  );
  const offline = agents.filter(
    (agent) => chipPresence(agent, paused) === "offline",
  );
  return [...connected, ...offline].slice(0, MAX_CHIPS);
}

/** The board filtered to the agent's tasks, through the board's `agent` search param. */
export function agentBoardPath(slug: string, agent: string): string {
  return withQuery(projectPath(slug, "board"), { agent });
}
