import { useMemo } from "react";
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
  const humanList = humans.data;
  const agentList = agents.data;
  // One object per change of the lists, so a Markdown body keyed on it parses again only then.
  const people = useMemo(
    () => ({
      humans: humanList ?? [],
      agents: agentList ? agentNames(agentList) : [],
    }),
    [humanList, agentList],
  );
  return slug === null ? NO_PEOPLE : people;
}
