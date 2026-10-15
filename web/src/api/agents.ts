import { api } from "./client";
import { expectOk, unwrap } from "./result";
import type { components } from "./schema";

type Schemas = components["schemas"];

/** One connected session as the session line words it. */
export type SessionView = Schemas["AgentSession"];
/** A stalled worker or a stale claim, by the task it is on. */
export type AgentAttention = Schemas["AgentAttentionItem"];
export type AgentView = Schemas["AgentEntry"];
export type WorkerView = Schemas["WorkerEntry"];
export type ClaimView = Schemas["HeldClaim"];
export type WorkspaceView = Schemas["WorkerWorkspace"];
export type ProposalView = Schemas["AuthoredProposal"];
export type DurableEvent = Schemas["DurableEvent"];

/** How many recent events the Agents screen asks for per agent. */
export const RECENT_EVENTS = 5;

/** Every agent of the project in registration order. `events` asks for that many newest events per agent. */
export async function listAgents(
  slug: string,
  events?: number,
): Promise<AgentView[]> {
  const { agents } = await unwrap(
    api.GET("/api/v1/projects/{slug}/agents", {
      params: { path: { slug }, query: { events } },
    }),
  );
  return agents;
}

/** Grants or removes the `integrator` tag. Rejects with the daemon's codes, such as `agent_not_found`. */
export async function setIntegrator(
  slug: string,
  agent: string,
  enabled: boolean,
): Promise<void> {
  const params = { params: { path: { slug, name: agent } } };
  await expectOk(
    enabled
      ? api.PUT("/api/v1/projects/{slug}/agents/{name}/tags/integrator", params)
      : api.DELETE(
          "/api/v1/projects/{slug}/agents/{name}/tags/integrator",
          params,
        ),
  );
}
