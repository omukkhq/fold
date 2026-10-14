import { ApiError } from "./errors";
import { fromFixture } from "./fixture";
import type { components } from "./schema";

export type ProjectSummary = components["schemas"]["ProjectSummary"];

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/projects")
  : undefined;

export async function listProjects(): Promise<ProjectSummary[]> {
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
