import { MAIN_TAG, type AgentView } from "@/api/agents";
import type { WorkerList } from "@/api/workers";
import { projectPath, withQuery } from "@/routes/paths";
import { areaOwner, MAIN_OWNER } from "./owners";

/** What decides where an agent's name leads: the area of each lead and the agents tagged as the main agent. */
export interface AgentLinks {
  /** Each lead's area, by the lead's agent name. */
  leads: ReadonlyMap<string, string>;
  main: ReadonlySet<string>;
}

export const NO_AGENT_LINKS: AgentLinks = { leads: new Map(), main: new Set() };

export function agentLinksOf(
  workers: WorkerList | undefined,
  agents: readonly AgentView[] | undefined,
): AgentLinks {
  const leads = new Map<string, string>();
  for (const area of workers?.areas ?? []) {
    if (area.lead !== null) leads.set(area.lead, area.name);
  }
  const main = new Set(
    (agents ?? [])
      .filter((agent) => agent.tags.includes(MAIN_TAG))
      .map((agent) => agent.name),
  );
  return { leads, main };
}

/** The board's `owner` search as the router writes a list into the URL. */
function ownerBoard(slug: string, owner: string): string {
  return withQuery(projectPath(slug, "board"), {
    owner: JSON.stringify([owner]),
  });
}

/**
 * A lead opens the board with its Owner entry ticked and the main agent with Main agent ticked, since the board filters by owner.
 * Any other agent (a task worker, a reviewer, the review lead) owns no board entry, so it opens the Agents screen.
 * A worker name such as `frontend/ui` in an older message stands for its agent.
 */
export function agentHref(
  slug: string,
  name: string,
  links: AgentLinks,
): string {
  const agent = name.split("/")[0] ?? name;
  const area = links.leads.get(agent);
  if (area !== undefined) return ownerBoard(slug, areaOwner(area));
  if (links.main.has(agent)) return ownerBoard(slug, MAIN_OWNER);
  return projectPath(slug, "agents");
}
