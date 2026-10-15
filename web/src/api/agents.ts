import { ApiError } from "./errors";
import { fromFixture } from "./fixture";
import type { components } from "./schema";
import type { Presence } from "../components/avatar/presence";

export type DurableEvent = components["schemas"]["DurableEvent"];
export type LiveWorkspace = components["schemas"]["LiveWorkspace"];

/** The OpenAPI schema has no `GET /agents` route, so these types are the payload the screen reads; each is declared here once and gives way to the generated type when the route is in the schema. */
export interface AttentionItem {
  kind: "stalled" | "stale_claim";
  /** @example T-12 */
  task: string;
}

export interface SessionView {
  id: string;
  /** UTC milliseconds. */
  started_at: number;
  /** UTC milliseconds. */
  last_call_at: number;
  pid: number;
  cwd: string;
}

export interface WorkerActivity {
  /** The tool the worker called last, for example `editing`. */
  tool: string;
  /** The path or id it called the tool on. */
  detail: string | null;
  /** UTC milliseconds. */
  at: number;
}

export interface WorkerView {
  /** `agent` or `agent/worker`. */
  label: string;
  /** The label of the parent worker; null for the agent's own worker. */
  parent: string | null;
  presence: Presence;
  /** UTC milliseconds; null before the first call. */
  last_call_at: number | null;
  activity: WorkerActivity | null;
  /** The task this worker holds a claim on. */
  claim: string | null;
}

export interface ClaimView {
  task: string;
  kind: "implementation" | "review";
  worker: string;
  /** The daemon's lease state as text. */
  lease: string;
  workspace: string | null;
  /** The task's open proposal for an implementation claim; the reviewed proposal for a review claim. */
  proposal: string | null;
}

export type WorkspaceRef = Pick<
  LiveWorkspace,
  "workspace" | "task" | "worker" | "summary" | "last_activity_at"
>;

export interface AgentView {
  name: string;
  tags: string[];
  presence: Presence;
  /** UTC milliseconds; null before the first call. */
  last_call_at: number | null;
  sessions: SessionView[];
  workers: WorkerView[];
  claims: ClaimView[];
  workspaces: WorkspaceRef[];
  events: DurableEvent[];
  attention: AttentionItem[];
  /** The human that granted the integrator tag; null when the agent has none. */
  integrator_granted_by: string | null;
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/agents")
  : undefined;

export async function listAgents(): Promise<AgentView[]> {
  return fromFixture(fixture, (m) => m.agents, []);
}

/**
 * Grants or removes the `integrator` tag. Rejects with the daemon's codes, such as `agent_not_found`.
 * Without a fixture it rejects with `not_found`, the daemon's answer to a route it does not serve, so the write never looks done.
 */
export async function setIntegrator(
  agent: string,
  enabled: boolean,
): Promise<void> {
  const done = await fromFixture(
    fixture,
    (m) => m.setIntegrator(agent, enabled),
    false,
  );
  if (!done) {
    throw new ApiError("not_found", "the daemon serves no agent tag route");
  }
}
