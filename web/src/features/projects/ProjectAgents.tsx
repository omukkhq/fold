import { Fragment } from "react";
import type { ProjectOverview } from "@/api/projects";
import { Tooltip } from "@/components/overlay/Tooltip";
import { PersonName } from "@/people/PersonName";
import { agentsLine } from "./projectsHome";

type Agent = ProjectOverview["agents"][number];

/** The connected agents of a project card as plain names, the first three and then how many more. The card is one link, so the names cannot take focus: a screen reader reads every name and a pointer gets them in the tooltip. */
export function ProjectAgents({ agents }: { agents: readonly Agent[] }) {
  if (agents.length === 0) {
    return <span className="text-text-2">No agents connected</span>;
  }
  const line = agentsLine(agents.map((agent) => agent.name));
  const names = (
    <span>
      <span aria-hidden={line.more === null ? undefined : true}>
        {line.named.map((name, index) => (
          <Fragment key={name}>
            {index > 0 ? ", " : null}
            <PersonName name={name} />
          </Fragment>
        ))}
        {line.more === null ? null : ` ${line.more}`}
      </span>
      {line.more === null ? null : <span className="sr-only">{line.all}</span>}
    </span>
  );
  return (
    <span className="flex min-w-0 items-start gap-8">
      <span
        aria-hidden="true"
        className="mt-7 size-8 shrink-0 rounded-full bg-ok"
      />
      {line.more === null ? (
        names
      ) : (
        <Tooltip content={line.all}>{names}</Tooltip>
      )}
    </span>
  );
}
