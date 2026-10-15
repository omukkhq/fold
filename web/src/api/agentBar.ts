import { queryOptions } from "@tanstack/react-query";
import { listAgents, type AgentView } from "./agents";
import { keys } from "./keys";

/** The top bar reads the same agent type as the Agents screen, without the recent events. */
export type ProjectAgent = AgentView;

/** The event stream keeps this fresh, so a cached answer never goes stale by time. */
export const agentBarQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.agentBar(slug),
    queryFn: () => listAgents(slug),
    staleTime: Infinity,
  });
