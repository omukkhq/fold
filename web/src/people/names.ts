const HUMAN_PREFIX = "human:";

export type PersonKind = "human" | "agent";

/** An actor arrives as `human:<name>` for a human; anything else (`frontend`, `fold`) is an agent. */
export function isHumanName(actor: string): boolean {
  return actor.startsWith(HUMAN_PREFIX);
}

export function personKind(actor: string): PersonKind {
  return isHumanName(actor) ? "human" : "agent";
}

/** The name a screen shows: a human by the name they signed in with, an agent by its full name. */
export function bareName(actor: string): string {
  return isHumanName(actor) ? actor.slice(HUMAN_PREFIX.length) : actor;
}

/** `@tanim`, `@frontend`: the person as plain text where markup is not possible, such as a placeholder. */
export function atName(actor: string): string {
  return `@${bareName(actor)}`;
}

/** A human reviewer the daemon holds as a bare name (`tanim`), as the actor `PersonName` takes; null when no one is named. */
export function reviewerActor(reviewer: string | null): string | null {
  return reviewer === null ? null : `${HUMAN_PREFIX}${reviewer}`;
}

/** The names a mention can point at. An agent's worker has the agent's name, so agents hold agent names (`frontend`). */
export interface People {
  humans: readonly string[];
  agents: readonly string[];
}

export const NO_PEOPLE: People = { humans: [], agents: [] };

/** What the name `x` of `@x` stands for. A human wins over an agent of the same name. */
export function personOf(people: People, name: string): PersonKind | null {
  if (name === "" || name.endsWith("/")) return null;
  if (people.humans.includes(name)) return "human";
  if (people.agents.includes(name)) return "agent";
  return null;
}

/** Every worker and agent name of an agents answer, for `People.agents`. */
export function agentNames(
  agents: readonly { name: string; workers: readonly { name: string }[] }[],
): string[] {
  return [
    ...new Set(
      agents.flatMap((agent) => [
        agent.name,
        ...agent.workers.map((worker) => worker.name),
      ]),
    ),
  ];
}
