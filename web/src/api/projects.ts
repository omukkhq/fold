import { ApiError } from "./errors";
import { fromFixture } from "./fixture";
import type { components } from "./schema";

export type ProjectSummary = components["schemas"]["ProjectSummary"];

/** The task count of each board column. */
export interface BoardCounts {
  backlog: number;
  todo: number;
  progress: number;
  review: number;
  done: number;
}

/** What the Projects screen draws beyond the schema's summary: the board counts, the active agents and the main head. The schema has no such fields yet. */
export interface ProjectOverview extends ProjectSummary {
  counts: BoardCounts;
  /** Names of the agents with work in progress. */
  agents: string[];
  /** The newest main revision number; null before the project has opened. */
  mainRevision: number | null;
  /** UTC milliseconds of the newest change; null when nothing has changed. */
  lastChangeAt: number | null;
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/projects")
  : undefined;

export async function listProjects(): Promise<ProjectOverview[]> {
  return fromFixture(fixture, (m) => m.projects, []);
}

/** Rejects with the daemon's codes: `project_not_found` for an unregistered slug, `project_unavailable` for a project that failed to open. */
export async function getProject(slug: string): Promise<ProjectSummary> {
  const found = (await listProjects()).find((p) => p.slug === slug);
  if (!found) {
    throw new ApiError(
      "project_not_found",
      `project ${slug} is not registered`,
    );
  }
  if (!found.available) {
    throw new ApiError("project_unavailable", `project ${slug} is unavailable`);
  }
  return found;
}
