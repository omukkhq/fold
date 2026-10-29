import type { ProjectAgent } from "../api/agentBar";
import type { LiveStatus } from "../live/status";
import { chipPresence, chipProcess, stalledCount } from "./agentChipsModel";

/** How many agents work now: those whose process works, or whose session is active. */
export function workingAgents(
  agents: readonly ProjectAgent[],
  paused: boolean,
): number {
  return agents.filter((agent) => chipPresence(agent, paused) === "active")
    .length;
}

export type AgentsTone = "live" | "connecting" | "reconnecting" | "paused";

export interface AgentsButtonView {
  label: string;
  tone: AgentsTone;
}

/** The top bar's Agents control: the pause before the stream, because a pause holds while the page reconnects; otherwise how many agents work. */
export function agentsButton(
  status: LiveStatus["state"],
  paused: boolean,
  working: number,
): AgentsButtonView {
  if (paused) return { label: "Agents paused", tone: "paused" };
  if (status === "reconnecting")
    return { label: "Reconnecting…", tone: "reconnecting" };
  const label =
    working === 0
      ? "No agents working"
      : `${String(working)} ${working === 1 ? "agent" : "agents"} working`;
  return { label, tone: status === "live" ? "live" : "connecting" };
}

/** The panel's dot: filled while the agent works, a ring while it is idle or not connected, red when its process failed. */
export type AgentDot = "work" | "idle" | "off" | "fail";

export interface AgentRowLook {
  dot: AgentDot;
  /** Gold for stalled work, red for a failed process or a model it cannot reach, else the secondary tone. */
  tone: "warn" | "bad" | null;
}

export function agentRowLook(
  agent: ProjectAgent,
  paused: boolean,
): AgentRowLook {
  const process = chipProcess(agent);
  const failed =
    process !== null &&
    (process.state === "failed" || process.model_unreachable);
  if (failed) return { dot: "fail", tone: "bad" };
  const presence = chipPresence(agent, paused);
  const dot: AgentDot =
    presence === "active" ? "work" : presence === "idle" ? "idle" : "off";
  return {
    dot,
    tone: stalledCount(agent) > 0 && !paused ? "warn" : null,
  };
}

/** The phone menu's agents row: how many work and how many there are. */
export function agentsSummary(working: number, total: number): string {
  return `${String(working)} working · ${String(total)} in all`;
}
