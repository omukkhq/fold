import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { agentBarQuery } from "@/api/agentBar";
import { workersQuery } from "@/api/queries";
import {
  agentLinksOf,
  NO_AGENT_LINKS,
  type AgentLinks,
} from "@/features/board/agentLink";

/** Where each agent's name leads, from the cached agents and workers lists. With no slug nothing is read. */
export function useAgentLinks(slug: string | null): AgentLinks {
  const agents = useQuery({
    ...agentBarQuery(slug ?? ""),
    enabled: slug !== null,
  });
  const workers = useQuery({
    ...workersQuery(slug ?? ""),
    enabled: slug !== null,
  });
  const links = useMemo(
    () => agentLinksOf(workers.data, agents.data),
    [workers.data, agents.data],
  );
  return slug === null ? NO_AGENT_LINKS : links;
}
