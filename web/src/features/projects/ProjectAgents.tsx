import { Fragment } from "react";
import type { ProjectOverview } from "@/api/projects";
import { PersonName } from "@/people/PersonName";
import { agentsLine } from "./projectsHome";

type Agent = ProjectOverview["agents"][number];

/** The connected agents of a project card as plain names, the first three and then how many more. The card is one link, so the names cannot take focus: a screen reader reads every name and a pointer gets them as hover text. */
export function ProjectAgents({ agents }: { agents: readonly Agent[] }) {
  if (agents.length === 0) {
    return <span className="text-text-2">No agents connected</span>;
  }
  const line = agentsLine(agents.map((agent) => agent.name));
  return (
    <span className="flex min-w-0 items-center gap-8">
      <span aria-hidden="true" className="size-8 shrink-0 rounded-full bg-ok" />
      <span title={line.more === null ? undefined : line.all}>
        <span aria-hidden={line.more === null ? undefined : true}>
          {line.named.map((name, index) => (
            <Fragment key={name}>
              {index > 0 ? ", " : null}
              <PersonName name={name} />
            </Fragment>
          ))}
          {line.more === null ? null : ` ${line.more}`}
        </span>
        {line.more === null ? null : (
          <span className="sr-only">{line.all}</span>
        )}
      </span>
    </span>
  );
}
