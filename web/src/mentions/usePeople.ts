import { useQuery } from "@tanstack/react-query";
import { agentBarQuery } from "@/api/agentBar";
import { humansQuery } from "@/api/humans";
import { NO_PEOPLE, agentNames, type People } from "@/people/names";

/** The names `@x` can mean, from the cached agents and humans lists. With no slug nothing is read and no name is known. */
export function usePeople(slug: string | null): People {
  const agents = useQuery({
    ...agentBarQuery(slug ?? ""),
    enabled: slug !== null,
  });
  const humans = useQuery({ ...humansQuery(), enabled: slug !== null });
  if (slug === null) return NO_PEOPLE;
  return {
    humans: humans.data ?? [],
    agents: agents.data ? agentNames(agents.data) : [],
  };
}
