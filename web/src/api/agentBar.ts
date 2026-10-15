import { queryOptions } from "@tanstack/react-query";
import type { Presence } from "../components/avatar/presence";
import { fromFixture } from "./fixture";
import { keys } from "./keys";

/** The schema has no agents route, so this is the one local declaration of what an agent chip reads. */
export interface ProjectAgent {
  name: string;
  presence: Presence;
  /** Tasks its workers hold an implementation claim on. */
  working: number;
  /** Of those, the ones whose worker is stalled. */
  stalled: number;
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/agentBar")
  : undefined;

export async function getProjectAgents(slug: string): Promise<ProjectAgent[]> {
  return fromFixture(fixture, (m) => m.agents(slug), []);
}

export const agentBarQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.agentBar(slug),
    queryFn: () => getProjectAgents(slug),
  });
